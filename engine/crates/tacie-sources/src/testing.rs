//! Fakes for tests (feature "testing"). They replace outside services with fakes at the
//! HTTP boundary. The engine launches no processes (only `gh auth token` in gh mode, `GhToken`), so there is no
//! process fake.
//!
//! # `FakeGitHub`
//!
//! A fake GitHub REST server (`testing::github`). Reply and record types are imported as
//! `testing::github::{FakeGitHub, Reply, Call}`. The gh mode token fakes (`FakeGhToken`, and `PanicGhToken`, which
//! panics when called) live there too.
//!
//! # `FakeSlack`
//!
//! A fake for Slack fetching (`SlackFetch`). `writes(raw)` writes that value to `raw/slack.json` on every fetch
//! as Python `json.dumps(ensure_ascii=False)` bytes (creating the folder), and `fails(message)` is
//! `SourceError::Other` with that text. `calls()` is the number of calls.
//!
//! # `FakeDropbox`
//!
//! A fake Dropbox (`testing::dropbox`). It holds folders, files, the change log, cursors and injected failures, and
//! mimics the log order seen on real Dropbox for renames, moves, unmounts and mounts.

pub mod asana;
pub mod atlassian;
pub mod dropbox;
pub mod gdrive;
pub mod github;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use tacie_index::pyjson;

use crate::error::SourceError;
use crate::slack::SlackFetch;

/// 가짜 슬랙 받기예요(모듈 설명의 `FakeSlack`).
pub struct FakeSlack {
    /// 쓸 원본, 또는 실패 글이에요.
    answer: Result<Value, String>,
    calls: AtomicUsize,
}

impl FakeSlack {
    /// 받을 때마다 `raw/slack.json`에 이 원본을 써요.
    pub fn writes(raw: Value) -> Arc<FakeSlack> {
        Arc::new(FakeSlack {
            answer: Ok(raw),
            calls: AtomicUsize::new(0),
        })
    }

    /// 받을 때마다 이 글로 실패해요.
    pub fn fails(message: &str) -> Arc<FakeSlack> {
        Arc::new(FakeSlack {
            answer: Err(message.to_string()),
            calls: AtomicUsize::new(0),
        })
    }

    /// 부른 수예요.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SlackFetch for FakeSlack {
    fn fetch(&self, raw: &Path) -> Result<(), SourceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match &self.answer {
            Ok(v) => {
                std::fs::create_dir_all(raw)?;
                std::fs::write(raw.join("slack.json"), pyjson::to_string(v, None))?;
                Ok(())
            }
            Err(message) => Err(SourceError::Other(message.clone())),
        }
    }
}
