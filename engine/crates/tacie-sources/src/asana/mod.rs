//! The Asana source (docs/32). It has REST calls (`api`), state and item files (`state`), fetching (`fetch`),
//! reconcile and the 30-minute check (`reconcile`), and chunks (`docs`).
//!
//! The token the app hands over is kept only in memory. The app refreshes it. A 401 during a fetch may mean "the token
//! is stale", so it sets `refused` to ask the app for a refresh and waits up to `wait` for a new token. When a new
//! token arrives it continues from the same request. If the new token also gets a 401, that is not a lost connection;
//! only this sync is `down` (`fetch::REFUSED_FRESH`). Only when no new token arrives after waiting (the app's refresh
//! was rejected with `invalid_grant`) does it record `lost_since`; after 7 days (`LOST_DAYS`) it stops searching but
//! keeps the raw data (`raw/asana/`) (same as Drive and Linear).

pub mod api;
mod docs;
mod fetch;
mod reconcile;
mod state;

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tacie_index::RawChunk;

use crate::error::{Kind, SourceError};
use crate::gdrive::api::TokenFn;
use crate::gdrive::fetch::{BATCH_TIME, Budget};
pub use crate::linear::Change;
use api::{API, AsanaApi, AsanaHttp, Pace, Reply};

pub use docs::{LOST_DAYS, chunks_of, scopes};

/// 받는 도중 401이 나면 앱이 갱신한 토큰을 이만큼 기다려요.
pub const TOKEN_WAIT: Duration = Duration::from_secs(90);

#[derive(Clone, PartialEq, Eq)]
struct Conn {
    token: String,
    user_gid: Option<String>,
}

type Shared = Arc<RwLock<Option<Conn>>>;

fn read(c: &Shared) -> Option<Conn> {
    c.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Asana 받기예요(갱신이 불러요).
pub trait AsanaFetch: Send + Sync {
    fn connected(&self) -> bool;
    /// 연결의 세대예요. 사용자를 바꾸거나 끊을 때마다 늘어요.
    fn generation(&self) -> u64;
    /// `raw` 아래를 새로 받아요. 몫이 모자라 다 받지 못했으면 `Ok(true)`예요.
    fn fetch(
        &self,
        raw: &Path,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<bool, SourceError>;
}

/// 진짜 Asana예요: 앱이 넘긴 토큰(메모리에만)으로 `AsanaHttp`를 불러요. 테스트는 `api`에 가짜를 넣어요.
pub struct AsanaSource {
    pub base: String,
    conn: Shared,
    generation: AtomicU64,
    refused: Arc<AtomicBool>,
    /// 지난 받기가 실패한 종류예요(`/index`의 `asana.error_kind`).
    error: Mutex<Option<Kind>>,
    /// 거절 뒤 앱이 갱신한 토큰으로 다시 받기를 건 때예요. 30분에 한 번만 걸어요.
    retried: Mutex<Option<Instant>>,
    api: Option<Arc<dyn AsanaApi>>,
    /// 401 뒤 새 토큰을 기다리는 시간이에요.
    wait: Duration,
    /// 새 토큰을 받았고 아직 그 토큰으로 받기가 끝나지 않았어요. 그동안 `/index`는 지난 `lost_since`를 보이지 않아요.
    untried: AtomicBool,
    /// 받기 한 번의 요청 몫이에요(`state::BUDGET_REQUESTS`, 테스트는 작게 줘요).
    requests: AtomicUsize,
    /// 이 Mac의 요청 속도예요. 받기와 30분 확인이 같이 써서 1분 한도를 함께 세고, 429 없이 10분이 지나면 한도가
    /// 올라가요(실행마다 새로 만들면 오르지 않아요).
    pace: Arc<Pace>,
}

impl Default for AsanaSource {
    fn default() -> Self {
        AsanaSource {
            base: API.to_string(),
            conn: Shared::default(),
            generation: AtomicU64::new(0),
            refused: Arc::default(),
            error: Mutex::default(),
            retried: Mutex::default(),
            api: None,
            wait: TOKEN_WAIT,
            untried: AtomicBool::new(false),
            requests: AtomicUsize::new(state::BUDGET_REQUESTS),
            pace: Pace::system(),
        }
    }
}

impl fmt::Debug for AsanaSource {
    /// 주소와 연결 여부만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AsanaSource")
            .field("base", &self.base)
            .field("connected", &self.connected())
            .finish_non_exhaustive()
    }
}

/// 401이 나면 앱의 새 토큰을 기다렸다 같은 요청을 한 번 더 하는 겉이에요(Atlassian과 같아요). 새 토큰으로도 401이면
/// 끊김(`key`)이 아니라 `fetch::REFUSED_FRESH`(`down`)이고 `lost_since`도 적지 않아요. 그 뒤 이번 동기화의 다른 401은
/// 기다리지 않아요(`gave_up`).
struct Fresh<'a> {
    inner: &'a dyn AsanaApi,
    conn: &'a Shared,
    refused: &'a AtomicBool,
    wait: Duration,
    gave_up: AtomicBool,
}

impl<'a> Fresh<'a> {
    fn new(
        inner: &'a dyn AsanaApi,
        conn: &'a Shared,
        refused: &'a AtomicBool,
        wait: Duration,
    ) -> Self {
        Fresh {
            inner,
            conn,
            refused,
            wait,
            gave_up: AtomicBool::new(false),
        }
    }
}

impl Fresh<'_> {
    fn again<T>(&self, call: impl Fn() -> Result<T, SourceError>) -> Result<T, SourceError> {
        let mut renewed_once = false;
        loop {
            let before = read(self.conn).map(|c| c.token);
            match call() {
                Err(e)
                    if e.kind() == Kind::Key
                        && before.is_some()
                        && (renewed_once || self.gave_up.load(Ordering::SeqCst)) =>
                {
                    // 앱의 새 토큰도 거절했어요: 토큰 탓이 아니라(권한, 설정) 이번만 실패예요
                    self.gave_up.store(true, Ordering::SeqCst);
                    return Err(SourceError::service(Kind::Down, fetch::REFUSED_FRESH));
                }
                Err(e) if e.kind() == Kind::Key && before.is_some() => {
                    // 앱이 `/index`의 `refused`를 보고 갱신해요
                    self.refused.store(true, Ordering::SeqCst);
                    let started = Instant::now();
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
                        return Err(e);
                    }
                    renewed_once = true;
                    self.refused.store(false, Ordering::SeqCst);
                }
                other => return other,
            }
        }
    }
}

impl AsanaApi for Fresh<'_> {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
        self.again(|| self.inner.get(path, query))
    }

    fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
        self.again(|| self.inner.batch(gets))
    }
}

impl AsanaSource {
    pub fn new(base: &str) -> Self {
        AsanaSource {
            base: base.to_string(),
            ..Default::default()
        }
    }

    /// 진짜 API 대신 `api`를 부르는 Asana예요(테스트). 401 뒤 기다리지 않아요.
    pub fn with_api(api: Arc<dyn AsanaApi>) -> Self {
        AsanaSource {
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

    /// 받기 한 번의 요청 몫을 바꿔요(테스트가 처음 받기를 몫으로 끊을 때 써요).
    #[doc(hidden)]
    pub fn set_budget(&self, requests: usize) {
        self.requests.store(requests, Ordering::SeqCst);
    }

    /// 연결을 바꿔요. 사용자(`user_gid`)가 바뀌면 `Switched`이고, 토큰만 바뀐 것은 `Rotated`라 세대를 올리지 않아요.
    pub fn set(&self, token: Option<String>, user_gid: Option<String>) -> Change {
        let new = token.map(|token| Conn { token, user_gid });
        let mut cur = self.conn.write().unwrap_or_else(PoisonError::into_inner);
        let change = match (&*cur, &new) {
            (None, None) => Change::Same,
            (None, Some(_)) => Change::Connected,
            (Some(_), None) => Change::Disconnected,
            (Some(a), Some(b)) if a.user_gid != b.user_gid => Change::Switched,
            (Some(a), Some(b)) if a == b => Change::Same,
            (Some(_), Some(_)) => Change::Rotated,
        };
        if change != Change::Same {
            self.refused.store(false, Ordering::SeqCst);
            *lock(&self.error) = None;
        }
        self.untried.store(
            matches!(
                change,
                Change::Connected | Change::Rotated | Change::Switched
            ) || (change == Change::Same && self.untried.load(Ordering::SeqCst)),
            Ordering::SeqCst,
        );
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

    /// 새 토큰으로 아직 받기가 끝나지 않았는지예요(`/index`가 지난 `lost_since`를 가려요).
    pub fn token_untried(&self) -> bool {
        self.untried.load(Ordering::SeqCst)
    }

    /// 거절 뒤 갱신한 토큰으로 다시 받기를 걸지예요: 30분에 한 번만 참이에요.
    pub fn retry_after_refusal(&self) -> bool {
        let mut at = lock(&self.retried);
        if at.is_some_and(|t| t.elapsed() < Duration::from_secs(30 * 60)) {
            return false;
        }
        *at = Some(Instant::now());
        true
    }

    /// 지난 받기가 실패한 종류예요. 성공했거나 아직 받지 않았으면 None이에요.
    pub fn error_kind(&self) -> Option<&'static str> {
        lock(&self.error).map(Kind::as_str)
    }

    /// 받기와 30분 확인이 같이 쓰는 요청 속도예요(테스트).
    #[doc(hidden)]
    pub fn pace(&self) -> &Arc<Pace> {
        &self.pace
    }

    /// 연결한 Asana 사용자예요.
    pub fn user_gid(&self) -> Option<String> {
        read(&self.conn).and_then(|c| c.user_gid)
    }

    fn token_fn(&self) -> TokenFn {
        let conn = self.conn.clone();
        Arc::new(move || read(&conn).map(|c| c.token))
    }

    /// 30분 확인이에요: 받을 변경이 있으면 참이에요. 연결이 없으면 None, 물을 수 없으면 Err예요. 한 번만, 짧게 물어요.
    pub fn changed(&self, raw: &Path, timeout: Duration) -> Option<Result<bool, SourceError>> {
        read(&self.conn)?;
        let http;
        let inner: &dyn AsanaApi = match &self.api {
            Some(api) => api.as_ref(),
            None => {
                http = AsanaHttp::with_tries(&self.base, self.token_fn(), 1, timeout)
                    .with_pace(self.pace.clone())
                    .give_up_after(reconcile::CHECK_TIME);
                &http
            }
        };
        // 401이면 받기와 같이 `refused`를 켜서 앱에 갱신을 부탁해요. 확인은 짧아야 해서 새 토큰을 기다리지 않아요.
        let api = Fresh::new(inner, &self.conn, &self.refused, Duration::ZERO);
        Some(reconcile::changed(&api, raw, Utc::now()))
    }
}

impl AsanaFetch for AsanaSource {
    fn connected(&self) -> bool {
        read(&self.conn).is_some()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn fetch(
        &self,
        raw: &Path,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<bool, SourceError> {
        let Some(conn) = read(&self.conn) else {
            return Err(SourceError::service(Kind::Key, "asana: not connected"));
        };
        let Some(user) = conn.user_gid.clone() else {
            return Err(SourceError::service(Kind::Key, "asana: no user"));
        };
        let g = self.generation();
        let still = || self.generation() == g;
        let http;
        let inner: &dyn AsanaApi = match &self.api {
            Some(api) => api.as_ref(),
            None => {
                http = AsanaHttp::new(&self.base, self.token_fn()).with_pace(self.pace.clone());
                &http
            }
        };
        let api = Fresh::new(inner, &self.conn, &self.refused, self.wait);
        let budget = Budget::new(self.requests.load(Ordering::SeqCst), BATCH_TIME);
        let got = fetch::fetch_budget(&api, raw, Utc::now(), &user, &still, progress, budget);
        if matches!(&got, Err(e) if e.kind() == Kind::Key) {
            self.refused.store(true, Ordering::SeqCst);
        }
        *lock(&self.error) = match &got {
            Err(e) => Some(e.kind()),
            // 새 토큰으로도 거절돼서 이번엔 건너뛰었어요
            Ok(f) if f.skipped => Some(Kind::Down),
            Ok(_) => None,
        };
        // 이 받기의 결과가 `lost_since`를 정해요(받기 중 새 토큰이 왔어도 그 토큰으로 받았어요)
        self.untried.store(false, Ordering::SeqCst);
        got.map(|f| f.more)
    }
}

fn saved(raw_dir: &Path) -> bool {
    raw_dir.join(state::STATE).exists() && raw_dir.join(state::ITEMS).exists()
}

/// 갱신의 Asana 몫이에요: 연결돼 있으면(그리고 `fetch`거나 저장본이 없으면) 받고, 저장본의 조각과 info
/// (`{workspaces, tasks, chunks, fetched_at, initial_done, lost_since, error_kind, blocked_workspaces}`), 그리고 다
/// 받지 못했는지(`more`)를 돌려줘요. `workspaces`는 막히지 않은 워크스페이스 수, `blocked_workspaces`는 막힌
/// 워크스페이스 이름이에요. 저장본이 없으면 조각과 info가 비어요. 받지 못했는데 저장본이 있으면 경고 `asana`를 남기고
/// 저장본으로 가요. 거절(`key`)이면 `lost_since`를 적고, 받기에 성공하면 지워요. `lost_since`에서 `LOST_DAYS`가
/// 지났으면 조각만 비고 원본은 남겨요.
pub fn gather(
    source: &dyn AsanaFetch,
    raw_dir: &Path,
    fetch: bool,
    progress: &mut dyn FnMut(usize, usize),
    warnings: &mut Vec<String>,
) -> Result<(Vec<RawChunk>, Option<Value>, bool), SourceError> {
    let mut failed: Option<Kind> = None;
    let mut more = false;
    if source.connected() && (fetch || !saved(raw_dir)) {
        let got = crate::gather::fetch_or_saved(
            "asana",
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
        match failed {
            // 기다리는 사이 연결이 지워졌으면(해제) 끊김이 아니에요.
            Some(Kind::Key) if source.connected() && s.lost_since.is_none() && saved(raw_dir) => {
                s.lost_since = Some(state::iso(Utc::now()));
                state::save_state(raw_dir, &s)?;
            }
            None if s.lost_since.is_some() => {
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
    let now = Utc::now();
    let s = {
        let _held = state::state_lock();
        state::load_state(raw_dir)
    };
    let chunks = chunks_of(raw_dir, now)?;
    let opt = |v: Option<String>| v.map_or(Value::Null, Value::from);
    let blocked: Vec<Value> = s
        .workspaces
        .iter()
        .filter(|w| w.blocked)
        .map(|w| Value::from(w.name.as_str()))
        .collect();
    let mut info = Map::new();
    info.insert(
        "workspaces".into(),
        Value::from(s.workspaces.iter().filter(|w| !w.blocked).count()),
    );
    info.insert(
        "tasks".into(),
        Value::from(state::item_ids(raw_dir, "task").len()),
    );
    info.insert("chunks".into(), Value::from(chunks.len()));
    info.insert("fetched_at".into(), opt(s.fetched_at.clone()));
    info.insert("initial_done".into(), Value::Bool(s.initial_done));
    info.insert("lost_since".into(), opt(s.lost_since.clone()));
    info.insert(
        "error_kind".into(),
        failed.map_or(Value::Null, |k| Value::from(k.as_str())),
    );
    info.insert("blocked_workspaces".into(), Value::Array(blocked));
    Ok((chunks, Some(Value::Object(info)), more))
}

/// 받은 지 `secs`초가 지났거나 받은 적이 없는지예요.
pub fn due(raw_dir: &Path, secs: i64) -> bool {
    let last = state::load_state(raw_dir)
        .fetched_at
        .and_then(|f| DateTime::parse_from_rfc3339(&f).ok());
    last.is_none_or(|l| Utc::now().timestamp() - l.timestamp() >= secs)
}

/// 저장본이 끊긴 연결로 적혀 있는지예요(`lost_since`).
pub fn stored_lost(raw_dir: &Path) -> bool {
    state::load_state(raw_dir).lost_since.is_some()
}

/// 저장본을 받은 Asana 사용자예요.
pub fn stored_user(raw_dir: &Path) -> Option<String> {
    state::load_state(raw_dir).user_gid
}

/// 저장본의 끈 워크스페이스·프로젝트(gid)예요. 저장본이 없으면 둘 다 비어요. 검색 데이터 지우기가 원본을 지운 뒤
/// `set_muted`로 되돌려 놓을 때 써요(끈 것이 풀려 AI로 가면 안 돼요).
pub fn muted(raw_dir: &Path) -> (Vec<String>, Vec<String>) {
    let _held = state::state_lock();
    let s = state::load_state(raw_dir);
    (s.muted_workspaces, s.muted_projects)
}

/// 끈 워크스페이스·프로젝트(gid)를 `state.json`에 적어요. 바뀌었으면 참이에요.
pub fn set_muted(
    raw_dir: &Path,
    workspaces: &[String],
    projects: &[String],
) -> Result<bool, SourceError> {
    let tidy = |xs: &[String]| {
        let mut v = xs.to_vec();
        v.sort();
        v.dedup();
        v
    };
    let (workspaces, projects) = (tidy(workspaces), tidy(projects));
    let _held = state::state_lock();
    let mut s = state::load_state(raw_dir);
    if s.muted_workspaces == workspaces && s.muted_projects == projects {
        return Ok(false);
    }
    s.muted_workspaces = workspaces;
    s.muted_projects = projects;
    state::save_state(raw_dir, &s)?;
    Ok(true)
}

/// 받은 Asana 자료를 모두 지워요(연결 해제, 다른 사용자).
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
    use crate::testing::asana::{FakeAsana, Task};

    /// 토큰 "good"으로 시작하는 것만 받아 주는 겉이에요. 안은 `FakeAsana`예요.
    struct Gate {
        fake: FakeAsana,
        conn: Mutex<Option<Shared>>,
        /// 어떤 토큰이든 401이에요(앱의 새 토큰도 거절해요).
        refuses: AtomicBool,
    }

    impl Gate {
        fn new() -> Arc<Gate> {
            let fake = FakeAsana::new();
            fake.set_now(Utc::now());
            fake.workspace("w1", "acme");
            fake.project("p1", "w1", "로드맵");
            fake.task(Task::new("1", "결제 웹훅").in_project("p1"));
            Arc::new(Gate {
                fake,
                conn: Mutex::new(None),
                refuses: AtomicBool::new(false),
            })
        }

        fn ok(&self) -> Result<(), SourceError> {
            let good = lock(&self.conn)
                .as_ref()
                .and_then(read)
                .is_some_and(|c| c.token.starts_with("good"));
            if good && !self.refuses.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(SourceError::service(Kind::Key, "asana: refused"))
            }
        }
    }

    impl AsanaApi for Gate {
        fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply, SourceError> {
            self.ok()?;
            self.fake.get(path, query)
        }
        fn batch(&self, gets: &[String]) -> Result<Vec<Reply>, SourceError> {
            self.ok()?;
            self.fake.batch(gets)
        }
    }

    fn source(gate: &Arc<Gate>) -> AsanaSource {
        let s = AsanaSource::with_api(gate.clone());
        *lock(&gate.conn) = Some(s.conn.clone());
        s
    }

    fn connect(s: &AsanaSource, token: &str) -> Change {
        s.set(Some(token.into()), Some("me-1".into()))
    }

    fn run(s: &AsanaSource, dir: &Path) -> (Vec<RawChunk>, Value, bool, Vec<String>) {
        let mut w = Vec::new();
        let (chunks, info, more) = gather(s, dir, true, &mut |_, _| {}, &mut w).unwrap();
        (chunks, info.unwrap_or(Value::Null), more, w)
    }

    /// `gather`의 결과예요.
    type Gathered = Result<(Vec<RawChunk>, Option<Value>, bool), SourceError>;

    /// 앱 노릇을 하며 한 번 받아요: 거절을 보면 새 토큰을 넘겨요(세 번까지). 넘긴 횟수도 돌려줘요.
    fn run_with_app(s: &Arc<AsanaSource>, dir: &Path) -> (Gathered, Vec<String>, usize) {
        let done = Arc::new(AtomicBool::new(false));
        let (s2, done2) = (s.clone(), done.clone());
        let app = std::thread::spawn(move || {
            let mut renewals = 0;
            while !done2.load(Ordering::SeqCst) && renewals < 3 {
                if s2.refused() {
                    renewals += 1;
                    connect(&s2, &format!("good{renewals}"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            renewals
        });
        let mut w = Vec::new();
        let got = gather(s.as_ref(), dir, true, &mut |_, _| {}, &mut w);
        done.store(true, Ordering::SeqCst);
        (got, w, app.join().unwrap())
    }

    #[test]
    fn a_fetch_reports_info_and_chunks() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "good");
        let (chunks, info, more, w) = run(&s, d.path());
        assert!(w.is_empty());
        assert!(!more);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["path"], "asana/task/1");
        assert_eq!(info["workspaces"], 1);
        assert_eq!(info["tasks"], 1);
        assert_eq!(info["chunks"], 1);
        assert_eq!(info["initial_done"], true);
        assert!(info["fetched_at"].is_string());
        assert!(info["lost_since"].is_null());
        assert!(info["error_kind"].is_null());
        assert_eq!(info["blocked_workspaces"], serde_json::json!([]));
        assert_eq!(stored_user(d.path()).as_deref(), Some("me-1"));
        assert!(!due(d.path(), 900));
        assert!(s.error_kind().is_none());
    }

    #[test]
    fn stale_token_waits_for_the_new_one() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = Arc::new(source(&gate).token_wait(Duration::from_secs(5)));
        connect(&s, "old");
        let s2 = s.clone();
        let renew = std::thread::spawn(move || {
            while !s2.refused() {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(connect(&s2, "good"), Change::Rotated);
        });
        let (chunks, info, _, w) = run(s.as_ref(), d.path());
        renew.join().unwrap();
        assert!(w.is_empty());
        assert_eq!(chunks.len(), 1);
        assert!(!s.refused());
        assert!(info["lost_since"].is_null());
        assert!(s.error_kind().is_none());
    }

    #[test]
    fn refused_fresh_token_is_down_not_lost() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = Arc::new(source(&gate).token_wait(Duration::from_secs(5)));
        connect(&s, "good");
        run_with_app(&s, d.path()).0.unwrap();
        // 앱의 갱신은 되지만 Asana가 새 토큰도 거절해요
        gate.refuses.store(true, Ordering::SeqCst);
        let (got, _, renewals) = run_with_app(&s, d.path());
        let (chunks, info, _) = got.unwrap();
        let info = info.unwrap();
        assert!(renewals <= 1, "waited for {renewals} new tokens");
        assert_eq!(chunks.len(), 1, "the saved copy is still searched");
        assert!(info["lost_since"].is_null());
        assert!(state::load_state(d.path()).lost_since.is_none());
        assert_eq!(s.error_kind(), Some("down"));
    }

    #[test]
    fn a_refusal_without_renewal_sets_lost_since_and_a_success_clears_it() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "good");
        run(&s, d.path());
        connect(&s, "revoked");
        let (chunks, info, _, w) = run(&s, d.path());
        assert!(s.refused());
        assert_eq!(w, ["asana"]);
        assert_eq!(chunks.len(), 1, "the saved copy is still searched");
        assert_eq!(info["error_kind"], "key");
        assert!(info["lost_since"].is_string());
        connect(&s, "good");
        assert!(s.token_untried());
        let (_, info, _, _) = run(&s, d.path());
        assert!(!s.token_untried());
        assert!(info["lost_since"].is_null());
        assert!(state::load_state(d.path()).lost_since.is_none());
    }

    #[test]
    fn seven_days_lost_hides_chunks_but_keeps_raw() {
        let d = tempfile::tempdir().unwrap();
        let raw = d.path().join("asana");
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "good");
        run(&s, &raw);
        let mut st = state::load_state(&raw);
        st.lost_since = Some(state::iso(Utc::now() - chrono::Duration::days(LOST_DAYS)));
        state::save_state(&raw, &st).unwrap();
        connect(&s, "revoked");
        let (chunks, info, _, _) = run(&s, &raw);
        assert!(chunks.is_empty());
        assert_eq!(info["chunks"], 0);
        assert!(info["lost_since"].is_string());
        assert!(raw.join("state.json").exists(), "raw/asana is kept");
        assert_eq!(state::item_ids(&raw, "task"), ["1"]);
        // 다시 연결하면 남은 원본에서 이어요.
        connect(&s, "good");
        let (chunks, info, _, _) = run(&s, &raw);
        assert_eq!(chunks.len(), 1);
        assert!(info["lost_since"].is_null());
    }

    #[test]
    fn a_disconnect_during_the_wait_is_not_a_lost_connection() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = Arc::new(source(&gate).token_wait(Duration::from_secs(5)));
        connect(&s, "good");
        run(&s, d.path());
        connect(&s, "old");
        let s2 = s.clone();
        let off = std::thread::spawn(move || {
            while !s2.refused() {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(s2.set(None, None), Change::Disconnected);
        });
        let mut w = Vec::new();
        gather(s.as_ref(), d.path(), true, &mut |_, _| {}, &mut w).unwrap();
        off.join().unwrap();
        assert_eq!(w, ["asana"]);
        assert!(state::load_state(d.path()).lost_since.is_none());
    }

    #[test]
    fn a_refused_check_asks_the_app_for_a_new_token() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate).token_wait(Duration::from_secs(60));
        connect(&s, "good");
        run(&s, d.path());
        connect(&s, "revoked");
        let started = Instant::now();
        let e = s
            .changed(d.path(), Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert_eq!(e.kind(), Kind::Key);
        assert!(s.refused());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_first_refusal_without_a_saved_copy_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "bad");
        let e = gather(&s, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap_err();
        assert_eq!(e.kind(), Kind::Key);
    }

    #[test]
    fn switched_user_is_reported() {
        let s = AsanaSource::default();
        assert_eq!(connect(&s, "a"), Change::Connected);
        let g = s.generation();
        assert_eq!(connect(&s, "b"), Change::Rotated);
        assert_eq!(s.generation(), g);
        assert_eq!(connect(&s, "b"), Change::Same);
        assert_eq!(
            s.set(Some("b".into()), Some("me-2".into())),
            Change::Switched
        );
        assert!(s.generation() > g);
        assert_eq!(s.user_gid().as_deref(), Some("me-2"));
        assert_eq!(s.set(None, None), Change::Disconnected);
        assert!(s.user_gid().is_none());
    }

    #[test]
    fn blocked_workspaces_are_named_in_info() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        gate.fake.workspace("w2", "beta");
        gate.fake.block_workspace("w2");
        let s = source(&gate);
        connect(&s, "good");
        let (_, info, _, _) = run(&s, d.path());
        assert_eq!(info["workspaces"], 1);
        assert_eq!(info["blocked_workspaces"], serde_json::json!(["beta"]));
    }

    #[test]
    fn mutes_and_forget_work_on_disk() {
        let d = tempfile::tempdir().unwrap();
        assert!(set_muted(d.path(), &["W".into(), "W".into()], &[]).unwrap());
        assert!(!set_muted(d.path(), &["W".into()], &[]).unwrap());
        assert!(set_muted(d.path(), &["W".into()], &["P".into()]).unwrap());
        let st = state::load_state(d.path());
        assert_eq!(st.muted_workspaces, ["W"]);
        assert_eq!(st.muted_projects, ["P"]);
        forget(d.path()).unwrap();
        assert!(!d.path().exists());
        forget(d.path()).unwrap();
    }

    #[test]
    fn debug_never_shows_the_token() {
        let s = AsanaSource::default();
        connect(&s, "secret_token");
        assert!(!format!("{s:?}").contains("secret"));
    }
}
