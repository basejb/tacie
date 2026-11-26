//! Dropbox (docs/33): fetches Paper, PDF, Office and text files from the last 2 years in folders the connected
//! person can see (team spaces included).
//!
//! The token the app hands over is kept only in memory. The app refreshes it (4-hour token). A 401 during a fetch may
//! mean "the token is stale", so it sets `refused` to ask the app for a refresh and waits up to `wait` for a new
//! token. When a new token arrives it continues fetching.

pub mod api;
pub mod cursor;
pub mod docs;
pub mod fetch;
pub mod reconcile;
pub mod state;

pub use fetch::WORKERS;

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tacie_index::RawChunk;

use crate::error::{Kind, SourceError};
use crate::gdrive::api::TokenFn;
use crate::gdrive::fetch::{BATCH_FILES, BATCH_TIME, Budget, iso};
pub use crate::linear::Change;
use api::{DropboxApi, DropboxHttp, Got};

pub use api::{API, CONTENT};
pub use docs::{FolderInfo, chunks_of, doc_paths_in_vaults, folders, web_link};
pub use state::{muted, set_muted};

/// 받는 도중 401이 나면 앱이 갱신한 토큰을 이만큼 기다려요.
pub const TOKEN_WAIT: Duration = Duration::from_secs(90);

#[derive(Clone, PartialEq, Eq)]
struct Conn {
    token: String,
    account: Option<String>,
    account_id: Option<String>,
}

type Shared = Arc<RwLock<Option<Conn>>>;

fn read(c: &Shared) -> Option<Conn> {
    c.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Dropbox 받기예요(갱신이 불러요).
pub trait DropboxFetch: Send + Sync {
    fn connected(&self) -> bool;
    /// 연결의 세대예요. 계정을 바꾸거나 끊을 때마다 늘어요.
    fn generation(&self) -> u64;
    fn account(&self) -> Option<String>;
    /// `raw` 아래를 새로 받아요. 몫이 차서 다 받지 못했으면 `Ok(true)`예요.
    fn fetch(
        &self,
        raw: &Path,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<bool, SourceError>;
}

/// 진짜 Dropbox예요: 앱이 넘긴 토큰(메모리에만)으로 `DropboxHttp`를 불러요. 테스트는 `api`에 가짜를 넣어요.
pub struct DropboxSource {
    pub api_base: String,
    pub content_base: String,
    conn: Shared,
    generation: AtomicU64,
    refused: Arc<AtomicBool>,
    /// 지난 받기가 실패한 종류예요(`/index`의 `dropbox.error_kind`). 성공하면 비워요.
    error: std::sync::Mutex<Option<Kind>>,
    /// 거절 뒤 앱이 갱신한 토큰으로 다시 받기를 건 때예요. 30분에 한 번만 걸어요.
    retried: std::sync::Mutex<Option<std::time::Instant>>,
    api: Option<Arc<dyn DropboxApi>>,
    /// 401 뒤 새 토큰을 기다리는 시간이에요.
    wait: Duration,
    /// 동기화 한 번의 받기 몫이에요.
    batch: std::sync::Mutex<(usize, Duration)>,
    /// 이 엔진에서 목록부터 받은 세대 + 1이에요(0은 아직). 엔진을 다시 띄우면 처음 받기를 끝내지 않은 저장본은
    /// 목록부터 다시 받아요(받는 도중 바뀐 것을 놓치지 않게요).
    listed: AtomicU64,
}

impl Default for DropboxSource {
    fn default() -> Self {
        DropboxSource {
            api_base: API.to_string(),
            content_base: CONTENT.to_string(),
            conn: Shared::default(),
            generation: AtomicU64::new(0),
            refused: Arc::default(),
            error: Default::default(),
            retried: Default::default(),
            api: None,
            wait: TOKEN_WAIT,
            batch: std::sync::Mutex::new((BATCH_FILES, BATCH_TIME)),
            listed: AtomicU64::new(0),
        }
    }
}

impl fmt::Debug for DropboxSource {
    /// 주소와 연결 여부만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DropboxSource")
            .field("api_base", &self.api_base)
            .field("content_base", &self.content_base)
            .field("connected", &self.connected())
            .finish_non_exhaustive()
    }
}

/// 401이 나면 앱의 새 토큰을 기다렸다 한 번 더 묻는 겉이에요.
struct Fresh<'a> {
    inner: &'a dyn DropboxApi,
    conn: &'a Shared,
    refused: &'a AtomicBool,
    wait: Duration,
    /// 받기를 시작한 연결 그대로인지예요. 끊거나 계정을 바꾼 뒤의 401로는 `refused`를 켜지 않아요.
    still: &'a (dyn Fn() -> bool + Sync),
}

impl Fresh<'_> {
    fn call(
        &self,
        f: impl Fn(&dyn DropboxApi) -> Result<Got, SourceError>,
    ) -> Result<Got, SourceError> {
        loop {
            let before = read(self.conn).map(|c| c.token);
            match f(self.inner) {
                Err(e) if e.kind() == Kind::Key && before.is_some() => {
                    if !(self.still)() {
                        return Err(e);
                    }
                    // 앱이 `/index`의 `refused`를 보고 갱신해요
                    self.refused.store(true, Ordering::SeqCst);
                    let started = std::time::Instant::now();
                    let step =
                        Duration::from_millis(200).min(self.wait.max(Duration::from_millis(1)));
                    let renewed = loop {
                        let now = read(self.conn).map(|c| c.token);
                        if now.is_none() {
                            break false;
                        }
                        if now != before {
                            break true;
                        }
                        if started.elapsed() >= self.wait {
                            break false;
                        }
                        std::thread::sleep(step);
                    };
                    if !renewed {
                        if !(self.still)() {
                            // 기다리는 사이 끊었어요: 옛 연결의 거절을 남기지 않아요
                            self.refused.store(false, Ordering::SeqCst);
                        }
                        return Err(e);
                    }
                    self.refused.store(false, Ordering::SeqCst);
                }
                other => return other,
            }
        }
    }
}

impl DropboxApi for Fresh<'_> {
    fn rpc(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
        self.call(|api| api.rpc(path, arg, root))
    }

    fn content(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
        self.call(|api| api.content(path, arg, root))
    }
}

impl DropboxSource {
    pub fn new(api: &str, content: &str) -> Self {
        DropboxSource {
            api_base: api.to_string(),
            content_base: content.to_string(),
            ..Default::default()
        }
    }

    /// 진짜 API 대신 `api`를 부르는 Dropbox예요(테스트). 401 뒤 기다리지 않아요.
    pub fn with_api(api: Arc<dyn DropboxApi>) -> Self {
        DropboxSource {
            api: Some(api),
            wait: Duration::ZERO,
            ..Default::default()
        }
    }

    /// 401 뒤 기다리는 시간을 바꿔요(테스트).
    pub fn token_wait(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    /// 동기화 한 번의 받기 몫을 바꿔요(테스트).
    pub fn set_batch(&self, files: usize, time: Duration) {
        *self.batch.lock().unwrap_or_else(PoisonError::into_inner) = (files, time);
    }

    /// 연결을 바꿔요. 토큰만 바뀐 것은 `Rotated`이고 세대를 올리지 않아요.
    pub fn set(
        &self,
        token: Option<String>,
        account: Option<String>,
        account_id: Option<String>,
    ) -> Change {
        let new = token.map(|token| Conn {
            token,
            account,
            account_id,
        });
        let mut cur = self.conn.write().unwrap_or_else(PoisonError::into_inner);
        let change = match (&*cur, &new) {
            (None, None) => Change::Same,
            (None, Some(_)) => Change::Connected,
            (Some(_), None) => Change::Disconnected,
            (Some(a), Some(b)) if a.account_id != b.account_id => Change::Switched,
            (Some(a), Some(b)) if a == b => Change::Same,
            (Some(_), Some(_)) => Change::Rotated,
        };
        if change != Change::Same {
            self.refused.store(false, Ordering::SeqCst);
            *self.error.lock().unwrap_or_else(PoisonError::into_inner) = None;
        }
        if matches!(
            change,
            Change::Switched | Change::Disconnected | Change::Connected
        ) {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        *cur = new;
        change
    }

    pub fn refused(&self) -> bool {
        self.refused.load(Ordering::SeqCst)
    }

    /// 거절 뒤 갱신한 토큰으로 다시 받기를 걸지예요: 30분에 한 번만 참이에요.
    pub fn retry_after_refusal(&self) -> bool {
        let mut at = self.retried.lock().unwrap_or_else(PoisonError::into_inner);
        if at.is_some_and(|t| t.elapsed() < Duration::from_secs(30 * 60)) {
            return false;
        }
        *at = Some(std::time::Instant::now());
        true
    }

    /// 지난 받기가 실패한 종류예요. 성공했거나 아직 받지 않았으면 None이에요.
    pub fn error_kind(&self) -> Option<&'static str> {
        self.error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(Kind::as_str)
    }

    pub fn account_id(&self) -> Option<String> {
        read(&self.conn).and_then(|c| c.account_id)
    }

    fn token_fn(&self) -> TokenFn {
        let conn = self.conn.clone();
        Arc::new(move || read(&conn).map(|c| c.token))
    }

    /// 30분 확인이에요: 받을 변경이 있으면 참이에요. 연결이 없으면 None, 물을 수 없으면 Err예요. 한 번만, 짧게 물어요.
    pub fn changed(&self, raw: &Path, timeout: Duration) -> Option<Result<bool, SourceError>> {
        read(&self.conn)?;
        let http;
        let api: &dyn DropboxApi = match &self.api {
            Some(api) => api.as_ref(),
            None => {
                http = DropboxHttp::with_tries(
                    &self.api_base,
                    &self.content_base,
                    self.token_fn(),
                    1,
                    timeout,
                );
                &http
            }
        };
        Some(cursor::changed(api, raw, Utc::now()))
    }
}

impl DropboxFetch for DropboxSource {
    fn connected(&self) -> bool {
        read(&self.conn).is_some()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn account(&self) -> Option<String> {
        read(&self.conn).and_then(|c| c.account)
    }

    /// 몫을 다 써서 멈췄을 때만 `Ok(true)`예요. `retry_error`로 남은 파일처럼 몫이 남았는데 받지 못한 것은 다음
    /// 동기화가 받아요(바로 다시 돌리면 같은 답만 받아요).
    fn fetch(
        &self,
        raw: &Path,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<bool, SourceError> {
        let Some(conn) = read(&self.conn) else {
            return Err(SourceError::service(Kind::Key, "dropbox: not connected"));
        };
        let g = self.generation();
        let still = || self.generation() == g;
        let http;
        let inner: &dyn DropboxApi = match &self.api {
            Some(api) => api.as_ref(),
            None => {
                http = DropboxHttp::new(&self.api_base, &self.content_base, self.token_fn());
                &http
            }
        };
        let api = Fresh {
            inner,
            conn: &self.conn,
            refused: &self.refused,
            wait: self.wait,
            still: &still,
        };
        let (files, time) = *self.batch.lock().unwrap_or_else(PoisonError::into_inner);
        let mut budget = Budget::new(files, time);
        let relist = self.listed.load(Ordering::SeqCst) != g + 1;
        let got = fetch::fetch_with(
            &api,
            raw,
            Utc::now(),
            conn.account_id.as_deref(),
            &still,
            relist,
            &mut budget,
            progress,
        )
        .map(|more| more && budget.spent());
        // 도중에 끊거나 계정을 바꿨으면 옛 연결의 결과(거절, 실패 종류)를 남기지 않아요
        if !still() {
            return got;
        }
        if matches!(&got, Err(e) if e.kind() == Kind::Key) {
            self.refused.store(true, Ordering::SeqCst);
        }
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) =
            got.as_ref().err().map(SourceError::kind);
        if got.is_ok() {
            self.listed.store(g + 1, Ordering::SeqCst);
        }
        got
    }
}

fn saved(raw_dir: &Path) -> bool {
    raw_dir.join(state::STATE).exists() && raw_dir.join(state::ITEMS).exists()
}

/// 갱신의 Dropbox 몫이에요: 연결돼 있으면(그리고 `fetch`거나 저장본이 없으면) 받고, 저장본의 조각과 info
/// (`{account, fetched_at, files, folders, chunks, initial_done, lost_since, error_kind}`), 그리고 다 받지 못했는지
/// (`more`)를 돌려줘요. 저장본이 없으면 조각과 info가 비어요. 받지 못했는데 저장본이 있으면 경고 `dropbox`를 남기고
/// 저장본으로 가요. 거절(`key`)이면 `lost_since`를 적고, 받기에 성공하면 지워요.
pub fn gather(
    source: &dyn DropboxFetch,
    raw_dir: &Path,
    fetch: bool,
    progress: &mut dyn FnMut(usize, usize),
    warnings: &mut Vec<String>,
) -> Result<(Vec<RawChunk>, Option<Value>, bool), SourceError> {
    let mut failed: Option<Kind> = None;
    let mut more = false;
    if source.connected() && (fetch || !saved(raw_dir)) {
        let g = source.generation();
        let got = crate::gather::fetch_or_saved(
            "dropbox",
            || match source.fetch(raw_dir, progress) {
                Ok(m) => {
                    more = m;
                    Ok(())
                }
                Err(e) => {
                    failed = Some(e.kind());
                    Err(e)
                }
            },
            || saved(raw_dir),
            warnings,
        );
        let held = state::state_lock();
        let mut s = state::load_state(raw_dir);
        // 받는 도중 끊거나 계정을 바꿨으면 `lost_since`를 건드리지 않아요
        let same = source.generation() == g;
        match failed.filter(|_| same) {
            Some(Kind::Key) if s.lost_since.is_none() && saved(raw_dir) => {
                s.lost_since = Some(iso(Utc::now()));
                state::save_state(raw_dir, &s)?;
            }
            None if same && s.lost_since.is_some() => {
                s.lost_since = None;
                state::save_state(raw_dir, &s)?;
            }
            _ => {}
        }
        drop(held);
        got?;
    }
    if !saved(raw_dir) {
        return Ok((Vec::new(), None, more));
    }
    let s = state::load_state(raw_dir);
    let chunks = chunks_of(raw_dir, Utc::now())?;
    let mut info = Map::new();
    info.insert(
        "account".into(),
        source
            .account()
            .or(s.account.clone())
            .map_or(Value::Null, Value::from),
    );
    info.insert(
        "fetched_at".into(),
        s.fetched_at.clone().map_or(Value::Null, Value::from),
    );
    info.insert("files".into(), Value::from(state::item_ids(raw_dir).len()));
    info.insert("folders".into(), Value::from(s.tops.len()));
    info.insert("chunks".into(), Value::from(chunks.len()));
    info.insert("initial_done".into(), Value::Bool(s.initial_done));
    info.insert(
        "lost_since".into(),
        s.lost_since.clone().map_or(Value::Null, Value::from),
    );
    info.insert(
        "error_kind".into(),
        failed.map_or(Value::Null, |k| Value::from(k.as_str())),
    );
    Ok((chunks, Some(Value::Object(info)), more))
}

/// 받은 지 `secs`초가 지났거나 받은 적이 없는지예요.
pub fn due(raw_dir: &Path, secs: i64) -> bool {
    let last = state::load_state(raw_dir)
        .fetched_at
        .and_then(|f| DateTime::parse_from_rfc3339(&f).ok());
    last.is_none_or(|l| Utc::now().timestamp() - l.timestamp() >= secs)
}

/// 저장본의 계정 id예요.
pub fn stored_account(raw_dir: &Path) -> Option<String> {
    state::load_state(raw_dir).account_id
}

/// 받은 Dropbox 자료를 모두 지워요(연결 해제, 다른 계정).
pub fn forget(raw_dir: &Path) -> std::io::Result<()> {
    let _held = state::state_lock();
    match std::fs::remove_dir_all(raw_dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::dropbox::FakeDropbox;
    use std::sync::Mutex;

    /// 토큰 "good"만 받아 주는 가짜예요. 그 뒤는 `FakeDropbox`(계정 `dbid:A`)가 답해요.
    struct Gate {
        conn: Mutex<Option<Shared>>,
        inner: FakeDropbox,
    }
    impl Gate {
        fn new() -> Arc<Gate> {
            let inner = FakeDropbox::new();
            inner.file("a", "/시험/A.md", 3, b"# A\nbody");
            Arc::new(Gate {
                conn: Mutex::new(None),
                inner,
            })
        }
        fn ok(&self) -> Result<(), SourceError> {
            let ok = self
                .conn
                .lock()
                .unwrap()
                .as_ref()
                .and_then(read)
                .is_some_and(|c| c.token == "good");
            if ok {
                Ok(())
            } else {
                Err(SourceError::service(Kind::Key, "dropbox: refused"))
            }
        }
    }
    impl DropboxApi for Gate {
        fn rpc(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
            self.ok()?;
            self.inner.rpc(path, arg, root)
        }
        fn content(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
            self.ok()?;
            self.inner.content(path, arg, root)
        }
    }

    fn source(gate: &Arc<Gate>) -> DropboxSource {
        let s = DropboxSource::with_api(gate.clone());
        *gate.conn.lock().unwrap() = Some(s.conn.clone());
        s
    }

    fn lists(gate: &Gate) -> usize {
        gate.inner
            .calls()
            .iter()
            .filter(|c| c.starts_with("rpc files/list_folder "))
            .count()
    }

    #[test]
    fn a_rotated_token_is_not_a_change_and_a_new_account_is() {
        let s = DropboxSource::default();
        assert_eq!(
            s.set(Some("a".into()), Some("me@x".into()), Some("dbid:A".into())),
            Change::Connected
        );
        let g = s.generation();
        assert_eq!(
            s.set(Some("b".into()), Some("me@x".into()), Some("dbid:A".into())),
            Change::Rotated
        );
        assert_eq!(s.generation(), g);
        assert_eq!(
            s.set(Some("b".into()), None, Some("dbid:B".into())),
            Change::Switched
        );
        assert!(s.generation() > g);
    }

    #[test]
    fn a_fetch_saves_the_account_and_reports_info() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        s.set(
            Some("good".into()),
            Some("me@x".into()),
            Some("dbid:A".into()),
        );
        let (chunks, info, more) =
            gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        assert!(!more);
        assert_eq!(chunks.len(), 1);
        let info = info.unwrap();
        assert_eq!(info["account"], "me@x");
        assert_eq!(info["files"], 1);
        assert_eq!(info["folders"], 1);
        assert_eq!(info["initial_done"], true);
        assert!(info["error_kind"].is_null());
        assert_eq!(stored_account(d.path()).as_deref(), Some("dbid:A"));
    }

    #[test]
    fn a_stale_token_waits_for_the_apps_renewal_and_carries_on() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = Arc::new(source(&gate).token_wait(Duration::from_secs(5)));
        s.set(Some("old".into()), None, Some("dbid:A".into()));
        let s2 = s.clone();
        let renew = std::thread::spawn(move || {
            while !s2.refused() {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                s2.set(Some("good".into()), None, Some("dbid:A".into())),
                Change::Rotated
            );
        });
        let (chunks, info, _) =
            gather(s.as_ref(), d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        renew.join().unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(!s.refused());
        assert!(info.unwrap()["lost_since"].is_null());
    }

    #[test]
    fn a_refusal_marks_refused_and_lost_since_and_a_success_clears_them() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        s.set(Some("good".into()), None, Some("dbid:A".into()));
        gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        s.set(Some("revoked".into()), None, Some("dbid:A".into()));
        let mut w = Vec::new();
        let (chunks, info, _) = gather(&s, d.path(), true, &mut |_, _| {}, &mut w).unwrap();
        assert!(s.refused());
        assert_eq!(w, ["dropbox"]);
        assert_eq!(s.error_kind(), Some("key"));
        assert!(!chunks.is_empty(), "the saved copy is still searched");
        let info = info.unwrap();
        assert_eq!(info["error_kind"], "key");
        assert!(info["lost_since"].is_string());
        s.set(Some("good".into()), None, Some("dbid:A".into()));
        gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        assert!(state::load_state(d.path()).lost_since.is_none());
        assert_eq!(s.error_kind(), None);
    }

    /// 부를 때마다 연결을 `next`로 바꾸고(끊기·계정 바꾸기) 401로 답하는 가짜예요.
    struct Cut {
        source: Mutex<std::sync::Weak<DropboxSource>>,
        next: Option<(&'static str, &'static str)>,
    }
    impl Cut {
        fn refuse(&self) -> Result<Got, SourceError> {
            if let Some(s) = self.source.lock().unwrap().upgrade() {
                let (token, id) = self.next.unzip();
                s.set(token.map(Into::into), None, id.map(Into::into));
            }
            Err(SourceError::service(Kind::Key, "dropbox: refused"))
        }
    }
    impl DropboxApi for Cut {
        fn rpc(&self, _: &str, _: &Value, _: Option<&str>) -> Result<Got, SourceError> {
            self.refuse()
        }
        fn content(&self, _: &str, _: &Value, _: Option<&str>) -> Result<Got, SourceError> {
            self.refuse()
        }
    }

    #[test]
    fn a_refusal_after_a_disconnect_or_switch_mid_fetch_leaves_no_stale_state() {
        for next in [None, Some(("other", "dbid:B"))] {
            let d = tempfile::tempdir().unwrap();
            state::save_state(d.path(), &state::State::default()).unwrap();
            std::fs::create_dir_all(d.path().join(state::ITEMS)).unwrap();
            let cut = Arc::new(Cut {
                source: Mutex::new(std::sync::Weak::new()),
                next,
            });
            let s = Arc::new(DropboxSource::with_api(cut.clone()));
            *cut.source.lock().unwrap() = Arc::downgrade(&s);
            s.set(Some("t".into()), None, Some("dbid:A".into()));
            let _ = gather(s.as_ref(), d.path(), true, &mut |_, _| {}, &mut Vec::new());
            assert!(!s.refused(), "{next:?}");
            assert_eq!(s.error_kind(), None, "{next:?}");
            assert!(state::load_state(d.path()).lost_since.is_none(), "{next:?}");
        }
    }

    #[test]
    fn a_first_refusal_without_a_saved_copy_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        s.set(Some("bad".into()), None, Some("dbid:A".into()));
        let e = gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap_err();
        assert_eq!(e.kind(), Kind::Key);
    }

    #[test]
    fn another_account_is_busy_and_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        s.set(Some("good".into()), None, Some("dbid:B".into()));
        let e = s.fetch(d.path(), &mut |_, _| {}).unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert!(!d.path().join(state::STATE).exists());
    }

    #[test]
    fn a_small_batch_reports_more() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        s.set_batch(0, Duration::from_secs(60));
        s.set(Some("good".into()), None, Some("dbid:A".into()));
        let (_, info, more) = gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        assert!(more);
        assert_eq!(info.unwrap()["initial_done"], false);
    }

    #[test]
    fn a_file_left_waiting_without_a_spent_batch_is_not_more() {
        // retry_error로 남은 파일은 다음 동기화가 받아요: 바로 다시 돌리지 않아요
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        gate.inner.blocked("a", "retry_error/..");
        let s = source(&gate);
        s.set(Some("good".into()), None, Some("dbid:A".into()));
        let (_, info, more) = gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        assert!(!more);
        assert_eq!(info.unwrap()["initial_done"], false);
        assert_eq!(state::load_state(d.path()).pending.len(), 1);
    }

    #[test]
    fn a_restart_before_the_first_fetch_ends_lists_again() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        gate.inner.file("b", "/시험/B.md", 2, b"# B\nbody");
        gate.inner.file("c", "/시험/C.md", 1, b"# C\nbody");
        let s = source(&gate);
        s.set_batch(1, Duration::from_secs(60));
        s.set(Some("good".into()), None, Some("dbid:A".into()));
        assert!(s.fetch(d.path(), &mut |_, _| {}).unwrap());
        assert_eq!(lists(&gate), 1);
        gate.inner.clear_calls();
        assert!(s.fetch(d.path(), &mut |_, _| {}).unwrap());
        assert_eq!(lists(&gate), 0, "the same source goes on from its cursor");
        assert!(
            gate.inner
                .calls()
                .iter()
                .any(|c| c == "rpc files/list_folder/continue")
        );
        gate.inner.clear_calls();
        let restarted = source(&gate);
        restarted.set_batch(1, Duration::from_secs(60));
        restarted.set(Some("good".into()), None, Some("dbid:A".into()));
        restarted.fetch(d.path(), &mut |_, _| {}).unwrap();
        assert_eq!(
            lists(&gate),
            1,
            "a new engine lists the unfinished first fetch again"
        );
    }

    #[test]
    fn forget_removes_everything() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        s.set(Some("good".into()), None, Some("dbid:A".into()));
        gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        forget(d.path()).unwrap();
        assert!(!d.path().exists());
        forget(d.path()).unwrap();
    }

    #[test]
    fn debug_never_shows_the_token() {
        let s = DropboxSource::default();
        s.set(Some("sl.secret_token".into()), None, None);
        assert!(!format!("{s:?}").contains("secret"));
    }

    #[test]
    fn due_follows_the_saved_fetch() {
        let d = tempfile::tempdir().unwrap();
        assert!(due(d.path(), 900));
        let gate = Gate::new();
        let s = source(&gate);
        s.set(Some("good".into()), None, Some("dbid:A".into()));
        gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        assert!(!due(d.path(), 900));
    }
}
