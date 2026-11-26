//! Jira and Confluence (Atlassian, docs/31). Both sources share the same account and the same text format (ADF).
//!
//! The token the app hands over is kept only in memory. The app refreshes it (1-hour token). A 401 during a fetch may
//! mean "the token is stale", so it sets `refused` to ask the app for a refresh and waits up to `wait` for a new
//! token. When a new token arrives it continues from the same request (same as Drive). Only when none arrives after
//! waiting does it record `lost_since`; after 7 days (`LOST_DAYS`), unlike Drive and Linear, it deletes even the raw
//! data (`raw/atlassian/`) (without a token it cannot send privacy reports either).

pub mod adf;
pub mod api;
mod confluence;
mod docs;
mod fetch;
mod jira;
mod privacy;
mod reconcile;
mod state;

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tacie_index::RawChunk;

use crate::error::{Kind, SourceError};
use crate::gdrive::api::{Got, TokenFn};
pub use crate::linear::Change;
use api::{API, Answer, AtlassianApi, AtlassianHttp};

pub use docs::{LOST_DAYS, chunks_of, scopes};

/// 받는 도중 401이 나면 앱이 갱신한 토큰을 이만큼 기다려요.
pub const TOKEN_WAIT: Duration = Duration::from_secs(90);

#[derive(Clone, PartialEq, Eq)]
struct Conn {
    token: String,
    cloud: Option<String>,
    site: Option<String>,
    site_url: Option<String>,
    /// The login email of an API token (the open edition): requests go to the site with Basic auth.
    email: Option<String>,
}

type Shared = Arc<RwLock<Option<Conn>>>;

fn read(c: &Shared) -> Option<Conn> {
    c.read().unwrap_or_else(PoisonError::into_inner).clone()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Atlassian 받기예요(갱신이 불러요).
pub trait AtlassianFetch: Send + Sync {
    fn connected(&self) -> bool;
    /// 연결의 세대예요. 사이트를 바꾸거나 끊을 때마다 늘어요.
    fn generation(&self) -> u64;
    /// `raw` 아래를 새로 받아요. 예산이 모자라 다 받지 못했으면 `Ok(true)`예요.
    fn fetch(
        &self,
        raw: &Path,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<bool, SourceError>;
    /// 사이트 이름이에요.
    fn site(&self) -> Option<String>;
}

/// 진짜 Atlassian이에요: 앱이 넘긴 토큰(메모리에만)으로 `AtlassianHttp`를 불러요. 테스트는 `api`에 가짜를 넣어요.
pub struct AtlassianSource {
    pub base: String,
    conn: Shared,
    generation: AtomicU64,
    refused: Arc<AtomicBool>,
    /// 지난 받기가 실패한 종류예요(`/index`의 `atlassian.error_kind`). 시간당 한도로 멈췄으면 `Busy`예요.
    error: Mutex<Option<Kind>>,
    /// 거절 뒤 앱이 갱신한 토큰으로 다시 받기를 건 때예요. 30분에 한 번만 걸어요.
    retried: Mutex<Option<Instant>>,
    /// 시간당 한도로 멈춘 때까지예요(지난 받기가 본 값).
    paused: Mutex<Option<String>>,
    /// `accessible-resources`를 읽은 연결 세대예요.
    resources_for: Mutex<Option<u64>>,
    api: Option<Arc<dyn AtlassianApi>>,
    /// 401 뒤 새 토큰을 기다리는 시간이에요.
    wait: Duration,
    /// 새 토큰을 받았고 아직 그 토큰으로 받기가 끝나지 않았어요. 그동안 `/index`는 지난 `lost_since`를 보이지 않아요
    /// (다시 연결한 바로 뒤에 "끊겼어요"가 남지 않게요). 저장본의 `lost_since`는 그대로라 7일 셈은 이어져요.
    untried: AtomicBool,
}

impl Default for AtlassianSource {
    fn default() -> Self {
        AtlassianSource {
            base: API.to_string(),
            conn: Shared::default(),
            generation: AtomicU64::new(0),
            refused: Arc::default(),
            error: Mutex::default(),
            retried: Mutex::default(),
            paused: Mutex::default(),
            resources_for: Mutex::default(),
            api: None,
            wait: TOKEN_WAIT,
            untried: AtomicBool::new(false),
        }
    }
}

impl fmt::Debug for AtlassianSource {
    /// 주소와 연결 여부만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtlassianSource")
            .field("base", &self.base)
            .field("connected", &self.connected())
            .finish_non_exhaustive()
    }
}

/// 401이 나면 앱의 새 토큰을 기다렸다 같은 요청을 한 번 더 하는 겉이에요. 새 토큰을 기다리는 건 요청마다 한
/// 번이에요. 새 토큰으로도 401이면 끊김(`key`)이 아니라 `fetch::REFUSED_FRESH`(`down`)라서 그 제품만 이번 동기화에서
/// 건너뛰고 `lost_since`도 적지 않아요. 그 뒤 이번 동기화의 다른 401은 기다리지 않아요(`gave_up`).
struct Fresh<'a> {
    inner: &'a dyn AtlassianApi,
    conn: &'a Shared,
    refused: &'a AtomicBool,
    wait: Duration,
    gave_up: AtomicBool,
}

impl<'a> Fresh<'a> {
    fn new(
        inner: &'a dyn AtlassianApi,
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
                    // 앱의 새 토큰도 거절했어요: 토큰 탓이 아니라(권한, 제품 설정) 이 요청만 실패예요
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

impl AtlassianApi for Fresh<'_> {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError> {
        self.again(|| self.inner.get(path, query))
    }

    fn post(&self, path: &str, body: &Value) -> Result<Answer, SourceError> {
        self.again(|| self.inner.post(path, body))
    }

    fn is_basic(&self) -> bool {
        self.inner.is_basic()
    }
}

impl AtlassianSource {
    pub fn new(base: &str) -> Self {
        AtlassianSource {
            base: base.to_string(),
            ..Default::default()
        }
    }

    /// 진짜 API 대신 `api`를 부르는 Atlassian이에요(테스트). 401 뒤 기다리지 않아요.
    pub fn with_api(api: Arc<dyn AtlassianApi>) -> Self {
        AtlassianSource {
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

    /// 연결을 바꿔요. 사이트(cloudid)가 바뀌면 `Switched`이고, 토큰이나 사이트 이름만 바뀐 것은 `Rotated`라 세대를
    /// 올리지 않아요.
    pub fn set(
        &self,
        token: Option<String>,
        cloud: Option<String>,
        site: Option<String>,
        site_url: Option<String>,
    ) -> Change {
        self.set_with_email(token, cloud, site, site_url, None)
    }

    /// `set` with the login email of an API token (the open edition pastes one instead of signing in).
    pub fn set_with_email(
        &self,
        token: Option<String>,
        cloud: Option<String>,
        site: Option<String>,
        site_url: Option<String>,
        email: Option<String>,
    ) -> Change {
        let new = token.map(|token| Conn {
            token,
            cloud,
            site,
            site_url,
            email,
        });
        let mut cur = self.conn.write().unwrap_or_else(PoisonError::into_inner);
        let change = match (&*cur, &new) {
            (None, None) => Change::Same,
            (None, Some(_)) => Change::Connected,
            (Some(_), None) => Change::Disconnected,
            (Some(a), Some(b)) if a.cloud != b.cloud => Change::Switched,
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
            *lock(&self.paused) = None;
        }
        *cur = new;
        change
    }

    /// The connection is an email and API token (the open edition), not an OAuth sign-in.
    pub fn uses_api_token(&self) -> bool {
        read(&self.conn).is_some_and(|c| c.email.is_some())
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

    /// 지난 받기가 실패한 종류예요. 시간당 한도로 멈춰 있는 동안은(토큰만 바뀌어도) `busy`이고, 성공했거나 아직 받지
    /// 않았으면 None이에요.
    pub fn error_kind(&self) -> Option<&'static str> {
        let paused = lock(&self.paused)
            .as_deref()
            .and_then(crate::gdrive::fetch::parse)
            .is_some_and(|t| t > Utc::now());
        if paused {
            return Some(Kind::Busy.as_str());
        }
        lock(&self.error).map(Kind::as_str)
    }

    pub fn cloud(&self) -> Option<String> {
        read(&self.conn).and_then(|c| c.cloud)
    }

    /// 시간당 한도로 멈춘 때까지예요(지난 받기가 본 값, RFC 3339).
    pub fn paused_until(&self) -> Option<String> {
        lock(&self.paused).clone()
    }

    /// The real API for the connection now: Basic against the site when it has an email, else Bearer via the gateway.
    fn http(&self, tries: u32, timeout: Duration) -> AtlassianHttp {
        let h = AtlassianHttp::with_tries(&self.base, self.token_fn(), tries, timeout);
        match read(&self.conn) {
            Some(Conn {
                email: Some(email),
                site_url: Some(site),
                ..
            }) => h.basic(&email, &site),
            _ => h,
        }
    }

    fn token_fn(&self) -> TokenFn {
        let conn = self.conn.clone();
        Arc::new(move || read(&conn).map(|c| c.token))
    }

    /// 30분 확인이에요: 받을 변경이 있으면 참이에요. 연결이 없으면 None, 물을 수 없으면 Err예요. 한 번만, 짧게 물어요.
    pub fn changed(&self, raw: &Path, timeout: Duration) -> Option<Result<bool, SourceError>> {
        read(&self.conn)?;
        let http;
        let inner: &dyn AtlassianApi = match &self.api {
            Some(api) => api.as_ref(),
            None => {
                http = self.http(1, timeout);
                &http
            }
        };
        // 401이면 받기와 같이 `refused`를 켜서 앱에 갱신을 부탁해요. 확인은 짧아야 해서 새 토큰을 기다리지 않아요.
        let api = Fresh::new(inner, &self.conn, &self.refused, Duration::ZERO);
        Some(reconcile::changed(&api, raw, Utc::now()))
    }
}

impl AtlassianFetch for AtlassianSource {
    fn connected(&self) -> bool {
        read(&self.conn).is_some()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn site(&self) -> Option<String> {
        read(&self.conn).and_then(|c| c.site)
    }

    fn fetch(
        &self,
        raw: &Path,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<bool, SourceError> {
        let Some(conn) = read(&self.conn) else {
            return Err(SourceError::service(Kind::Key, "atlassian: not connected"));
        };
        let Some(cloud) = conn.cloud.clone() else {
            return Err(SourceError::service(Kind::Key, "atlassian: no site"));
        };
        let g = self.generation();
        let still = || self.generation() == g;
        let http;
        let inner: &dyn AtlassianApi = match &self.api {
            Some(api) => api.as_ref(),
            None => {
                http = self.http(api::TRIES, api::TIMEOUT);
                &http
            }
        };
        let api = Fresh::new(inner, &self.conn, &self.refused, self.wait);
        let site = fetch::Site {
            cloud: &cloud,
            site: conn.site.as_deref(),
            site_url: conn.site_url.as_deref(),
            load_products: *lock(&self.resources_for) != Some(g),
        };
        let got = fetch::fetch_with(
            &api,
            raw,
            Utc::now(),
            &site,
            &still,
            &state::jitter,
            progress,
        );
        if matches!(&got, Err(e) if e.kind() == Kind::Key) {
            self.refused.store(true, Ordering::SeqCst);
        }
        if let Ok(f) = &got
            && f.resources
            && still()
        {
            *lock(&self.resources_for) = Some(g);
        }
        *lock(&self.error) = match &got {
            Err(e) => Some(e.kind()),
            Ok(f) if f.halted => Some(Kind::Busy),
            // 새 토큰으로도 거절된 제품을 건너뛰었어요
            Ok(f) if f.skipped => Some(Kind::Down),
            Ok(_) => None,
        };
        *lock(&self.paused) = fetch::paused_at(raw, Utc::now());
        // 이 받기의 결과가 `lost_since`를 정해요(받기 중 새 토큰이 왔어도 그 토큰으로 받았어요)
        self.untried.store(false, Ordering::SeqCst);
        got.map(|f| f.more)
    }
}

fn saved(raw_dir: &Path) -> bool {
    raw_dir.join(state::STATE).exists() && raw_dir.join(state::ITEMS).exists()
}

/// 갱신의 Atlassian 몫이에요: 연결돼 있으면(그리고 `fetch`거나 저장본이 없으면) 받고, 저장본의 조각과 info
/// (`{site, site_url, fetched_at, issues, pages, chunks, initial_done, lost_since, paused_until, error_kind}`), 그리고
/// 다 받지 못했는지(`more`)를 돌려줘요. 저장본이 없으면 조각과 info가 비어요. 받지 못했는데 저장본이 있으면 경고
/// `atlassian`을 남기고 저장본으로 가요. 거절(`key`)이면 `lost_since`를 적고, 받기에 성공하면 지워요. `lost_since`에서
/// `LOST_DAYS`가 지났으면 원본을 지우고 빈 채로 돌려줘요.
pub fn gather(
    source: &dyn AtlassianFetch,
    raw_dir: &Path,
    fetch: bool,
    progress: &mut dyn FnMut(usize, usize),
    warnings: &mut Vec<String>,
) -> Result<(Vec<RawChunk>, Option<Value>, bool), SourceError> {
    let mut failed: Option<Kind> = None;
    let mut more = false;
    if source.connected() && (fetch || !saved(raw_dir)) {
        let got = crate::gather::fetch_or_saved(
            "atlassian",
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
    let s = state::load_state(raw_dir);
    if docs::expired(&s, now) {
        forget(raw_dir)?;
        return Ok((Vec::new(), None, more));
    }
    let chunks = chunks_of(raw_dir, now)?;
    let paused = s
        .paused_until
        .clone()
        .filter(|t| crate::gdrive::fetch::parse(t).is_some_and(|t| t > now));
    let pages = state::item_ids(raw_dir, "page")
        .iter()
        .filter(|id| state::read_item(raw_dir, "page", id).is_some_and(|v| v["page"].is_object()))
        .count();
    let opt = |v: Option<String>| v.map_or(Value::Null, Value::from);
    let error_kind = failed
        .or(paused.as_ref().map(|_| Kind::Busy))
        .map_or(Value::Null, |k| Value::from(k.as_str()));
    let mut info = Map::new();
    info.insert("site".into(), opt(source.site().or(s.site.clone())));
    info.insert("site_url".into(), opt(s.site_url.clone()));
    info.insert("fetched_at".into(), opt(s.fetched_at.clone()));
    info.insert(
        "issues".into(),
        Value::from(state::item_ids(raw_dir, "jira").len()),
    );
    info.insert("pages".into(), Value::from(pages));
    info.insert("chunks".into(), Value::from(chunks.len()));
    info.insert(
        "initial_done".into(),
        Value::Bool(fetch::initial_done(raw_dir)),
    );
    info.insert("lost_since".into(), opt(s.lost_since.clone()));
    info.insert("paused_until".into(), opt(paused));
    info.insert("error_kind".into(), error_kind);
    Ok((chunks, Some(Value::Object(info)), more))
}

/// 받은 지 `secs`초가 지났거나 받은 적이 없는지예요.
pub fn due(raw_dir: &Path, secs: i64) -> bool {
    let last = state::load_state(raw_dir)
        .fetched_at
        .and_then(|f| DateTime::parse_from_rfc3339(&f).ok());
    last.is_none_or(|l| Utc::now().timestamp() - l.timestamp() >= secs)
}

/// 저장본의 사이트(cloudid)예요.
pub fn stored_cloud(raw_dir: &Path) -> Option<String> {
    state::load_state(raw_dir).cloud
}

/// 저장본의 끈 프로젝트·스페이스(id나 key)예요. 저장본이 없으면 둘 다 비어요. 검색 데이터 지우기가 원본을 지운 뒤
/// `set_muted`로 되돌려 놓을 때 써요(끈 것이 풀려 AI로 가면 안 돼요).
pub fn muted(raw_dir: &Path) -> (Vec<String>, Vec<String>) {
    let _held = state::state_lock();
    let s = state::load_state(raw_dir);
    (s.muted_projects, s.muted_spaces)
}

/// 끈 프로젝트·스페이스(id나 key)를 `state.json`에 적어요. 바뀌었으면 참이에요.
pub fn set_muted(
    raw_dir: &Path,
    projects: &[String],
    spaces: &[String],
) -> Result<bool, SourceError> {
    let tidy = |xs: &[String]| {
        let mut v = xs.to_vec();
        v.sort();
        v.dedup();
        v
    };
    let (projects, spaces) = (tidy(projects), tidy(spaces));
    let _held = state::state_lock();
    let mut s = state::load_state(raw_dir);
    if s.muted_projects == projects && s.muted_spaces == spaces {
        return Ok(false);
    }
    s.muted_projects = projects;
    s.muted_spaces = spaces;
    state::save_state(raw_dir, &s)?;
    Ok(true)
}

/// 받은 Atlassian 자료를 모두 지워요(연결 해제, 다른 사이트, 끊긴 지 7일).
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
    use crate::testing::atlassian::FakeAtlassian;
    use serde_json::json;

    /// 토큰 "good"만 받아 주는 겉이에요. 안은 `FakeAtlassian`이에요.
    struct Gate {
        fake: FakeAtlassian,
        conn: Mutex<Option<Shared>>,
        /// Jira API는 어떤 토큰이든 401이에요(앱의 새 토큰도 거절해요).
        jira_refuses: AtomicBool,
        /// Confluence API도 어떤 토큰이든 401이에요.
        wiki_refuses: AtomicBool,
    }

    impl Gate {
        fn new() -> Arc<Gate> {
            let fake = FakeAtlassian::new();
            *fake.jira.lock().unwrap() = vec![json!({
                "id": "10", "key": "ENG-1",
                "fields": {"summary": "Login breaks", "updated": jira_now(),
                           "project": {"id": "1", "key": "ENG", "name": "Engine"}}
            })];
            Arc::new(Gate {
                fake,
                conn: Mutex::new(None),
                jira_refuses: AtomicBool::new(false),
                wiki_refuses: AtomicBool::new(false),
            })
        }

        fn ok(&self, path: &str) -> Result<(), SourceError> {
            if (self.jira_refuses.load(Ordering::SeqCst) && path.contains("/rest/api/3/"))
                || (self.wiki_refuses.load(Ordering::SeqCst) && path.contains("/wiki/"))
            {
                return Err(SourceError::service(Kind::Key, "atlassian: refused"));
            }
            let good = lock(&self.conn)
                .as_ref()
                .and_then(read)
                .is_some_and(|c| c.token.starts_with("good"));
            if good {
                Ok(())
            } else {
                Err(SourceError::service(Kind::Key, "atlassian: refused"))
            }
        }

        fn calls(&self) -> usize {
            self.fake.calls().len()
        }
    }

    impl AtlassianApi for Gate {
        fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError> {
            self.ok(path)?;
            self.fake.get(path, query)
        }
        fn post(&self, path: &str, body: &Value) -> Result<Answer, SourceError> {
            self.ok(path)?;
            self.fake.post(path, body)
        }
    }

    fn jira_now() -> String {
        (Utc::now() - chrono::Duration::hours(1))
            .format("%Y-%m-%dT%H:%M:%S%.3f+0000")
            .to_string()
    }

    fn source(gate: &Arc<Gate>) -> AtlassianSource {
        let s = AtlassianSource::with_api(gate.clone());
        *lock(&gate.conn) = Some(s.conn.clone());
        s
    }

    fn connect(s: &AtlassianSource, token: &str) -> Change {
        s.set(
            Some(token.into()),
            Some("cloud-1".into()),
            Some("acme".into()),
            Some("https://acme.atlassian.net".into()),
        )
    }

    fn run(s: &AtlassianSource, dir: &Path) -> (Vec<RawChunk>, Value, bool, Vec<String>) {
        let mut w = Vec::new();
        let (chunks, info, more) = gather(s, dir, true, &mut |_, _| {}, &mut w).unwrap();
        (chunks, info.unwrap_or(Value::Null), more, w)
    }

    #[test]
    fn a_fetch_reports_info_and_chunks() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "good");
        let (chunks, info, more, _) = run(&s, d.path());
        assert!(!more);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["path"], "jira/cloud-1/10");
        assert_eq!(info["site"], "acme");
        assert_eq!(info["site_url"], "https://acme.atlassian.net");
        assert_eq!(info["issues"], 1);
        assert_eq!(info["pages"], 0);
        assert_eq!(info["initial_done"], true);
        assert!(info["paused_until"].is_null());
        assert!(info["error_kind"].is_null());
        assert_eq!(stored_cloud(d.path()).as_deref(), Some("cloud-1"));
        assert!(!due(d.path(), 900));
        // Confluence가 없는 사이트라 Confluence를 부르지 않아요.
        assert!(gate.fake.calls().iter().all(|c| !c.contains("/wiki/")));
    }

    #[test]
    fn a_stale_token_waits_for_the_apps_renewal_and_carries_on() {
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
    fn a_refusal_without_renewal_sets_lost_since_and_seven_days_later_raw_is_gone() {
        let d = tempfile::tempdir().unwrap();
        let raw = d.path().join("atlassian");
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "good");
        run(&s, &raw);
        connect(&s, "revoked");
        let (chunks, info, _, w) = run(&s, &raw);
        assert!(s.refused());
        assert_eq!(w, ["atlassian"]);
        assert!(!chunks.is_empty(), "the saved copy is still searched");
        assert_eq!(info["error_kind"], "key");
        assert!(info["lost_since"].is_string());
        // 7일이 지났어요.
        let mut st = state::load_state(&raw);
        st.lost_since = Some(state::iso(Utc::now() - chrono::Duration::days(LOST_DAYS)));
        state::save_state(&raw, &st).unwrap();
        let (chunks, info, _, _) = run(&s, &raw);
        assert!(chunks.is_empty());
        assert!(info.is_null());
        assert!(!raw.exists(), "raw/atlassian is deleted");
        // 같은 사이트로 다시 연결하면(세대가 그대로예요) 제품을 다시 읽고 처음부터 받아요.
        let g = s.generation();
        assert_eq!(connect(&s, "good"), Change::Rotated);
        assert_eq!(s.generation(), g);
        let (chunks, info, _, _) = run(&s, &raw);
        assert_eq!(chunks.len(), 1);
        assert_eq!(info["issues"], 1);
        assert!(info["lost_since"].is_null());
    }

    /// Jira와 Confluence가 다 있고 Confluence 글이 하나 있는 사이트예요.
    fn both_products() -> Arc<Gate> {
        let gate = Gate::new();
        *gate.fake.resources.lock().unwrap() = vec![json!({
            "id": "cloud-1", "url": "https://acme.atlassian.net", "name": "acme",
            "scopes": ["read:jira-work", "read:page:confluence"]
        })];
        let t = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        *gate.fake.pages.lock().unwrap() = vec![json!({
            "id": "7", "status": "current", "title": "Auth design", "spaceId": "9",
            "authorId": "u-author", "version": {"number": 1, "createdAt": t, "authorId": "u-author"},
            "body": {"atlas_doc_format": {"value": "{\"type\":\"doc\",\"content\":[]}"}},
            "_links": {"webui": "/spaces/ENG/pages/7"}
        })];
        gate
    }

    /// `gather`의 결과예요.
    type Gathered = Result<(Vec<RawChunk>, Option<Value>, bool), SourceError>;

    /// 앱 노릇을 하며 한 번 받아요: 거절을 보면 새 토큰을 넘겨요. 끝없이 돌면 세 번에서 멈춰요. 새 토큰을 넘긴
    /// 횟수도 돌려줘요.
    fn run_with_app(s: &Arc<AtlassianSource>, dir: &Path) -> (Gathered, Vec<String>, usize) {
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
    fn a_refusal_even_with_a_fresh_token_skips_that_product_once() {
        let d = tempfile::tempdir().unwrap();
        let gate = both_products();
        gate.jira_refuses.store(true, Ordering::SeqCst);
        let s = Arc::new(source(&gate).token_wait(Duration::from_secs(5)));
        connect(&s, "good");
        let (got, _, renewals) = run_with_app(&s, d.path());
        let (chunks, info, _) = got.unwrap();
        let info = info.unwrap();
        assert!(renewals <= 1, "waited for {renewals} new tokens");
        assert!(info["lost_since"].is_null());
        assert!(state::load_state(d.path()).lost_since.is_none());
        assert_eq!(info["pages"], 1, "Confluence is still fetched");
        assert!(chunks.iter().any(|c| c["src"] == "confluence"));
        assert_eq!(s.error_kind(), Some("down"));
    }

    #[test]
    fn jira_fine_and_confluence_refused_keeps_jira() {
        let d = tempfile::tempdir().unwrap();
        let gate = both_products();
        gate.wiki_refuses.store(true, Ordering::SeqCst);
        let s = Arc::new(source(&gate).token_wait(Duration::from_secs(5)));
        connect(&s, "good");
        let (got, _, renewals) = run_with_app(&s, d.path());
        let (chunks, info, _) = got.unwrap();
        let info = info.unwrap();
        assert!(renewals <= 1, "waited for {renewals} new tokens");
        assert!(info["lost_since"].is_null());
        assert_eq!(info["issues"], 1, "Jira is still fetched");
        assert!(chunks.iter().any(|c| c["src"] == "jira"));
        assert_eq!(s.error_kind(), Some("down"));
    }

    #[test]
    fn every_product_refused_with_a_fresh_token_is_a_lost_site() {
        let d = tempfile::tempdir().unwrap();
        let gate = both_products();
        let s = Arc::new(source(&gate).token_wait(Duration::from_secs(5)));
        connect(&s, "good");
        run_with_app(&s, d.path()).0.unwrap();
        // 관리자가 앱을 뺐어요: 앱의 갱신은 되지만 사이트가 모든 요청을 거절해요
        gate.jira_refuses.store(true, Ordering::SeqCst);
        gate.wiki_refuses.store(true, Ordering::SeqCst);
        // 끝난 뒤 `key`가 앱에 갱신을 다시 부탁하니 넘긴 횟수는 보지 않아요(기다림은 위 두 시험이 봐요)
        let (got, w, _) = run_with_app(&s, d.path());
        let (chunks, info, _) = got.unwrap();
        let info = info.unwrap();
        assert_eq!(w, ["atlassian"]);
        assert!(!chunks.is_empty(), "the saved copy is still searched");
        assert_eq!(info["error_kind"], "key");
        assert!(info["lost_since"].is_string());
        assert!(state::load_state(d.path()).lost_since.is_some());
    }

    #[test]
    fn a_new_token_hides_lost_since_until_it_is_tried() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        assert!(!s.token_untried());
        connect(&s, "good");
        assert!(s.token_untried());
        run(&s, d.path());
        assert!(!s.token_untried());
        connect(&s, "revoked");
        run(&s, d.path());
        let since = state::load_state(d.path()).lost_since;
        assert!(since.is_some());
        // 다시 연결: 새 토큰이 오면 받기 전까지 가리고, 저장본의 날짜는 건드리지 않아요
        connect(&s, "good");
        assert!(s.token_untried());
        assert_eq!(state::load_state(d.path()).lost_since, since);
        connect(&s, "good");
        assert!(s.token_untried(), "the same token again keeps it hidden");
        run(&s, d.path());
        assert!(!s.token_untried());
        assert!(state::load_state(d.path()).lost_since.is_none());
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
            assert_eq!(s2.set(None, None, None, None), Change::Disconnected);
        });
        let mut w = Vec::new();
        gather(s.as_ref(), d.path(), true, &mut |_, _| {}, &mut w).unwrap();
        off.join().unwrap();
        assert_eq!(w, ["atlassian"]);
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
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the check does not wait"
        );
    }

    #[test]
    fn a_success_clears_lost_since() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "good");
        run(&s, d.path());
        connect(&s, "revoked");
        run(&s, d.path());
        assert!(state::load_state(d.path()).lost_since.is_some());
        connect(&s, "good");
        run(&s, d.path());
        assert!(state::load_state(d.path()).lost_since.is_none());
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
    fn quota_pause_is_busy_not_lost() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "good");
        run(&s, d.path());
        let reset = Utc::now() + chrono::Duration::hours(1);
        *gate.fake.quota.lock().unwrap() =
            Some(reset.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        let (chunks, info, more, w) = run(&s, d.path());
        assert!(w.is_empty());
        assert!(!more);
        assert_eq!(chunks.len(), 1, "the saved copy is still searched");
        assert_eq!(s.error_kind(), Some("busy"));
        assert!(!s.refused());
        assert!(info["lost_since"].is_null());
        assert!(info["paused_until"].is_string());
        assert_eq!(info["error_kind"], "busy");
        assert_eq!(
            s.paused_until(),
            info["paused_until"].as_str().map(str::to_string)
        );
        // 토큰만 바뀌어도 멈춰 있는 동안은 `busy`예요.
        assert_eq!(
            s.set(
                Some("good".into()),
                Some("cloud-1".into()),
                Some("acme".into()),
                Some("https://acme.atlassian.net/".into()),
            ),
            Change::Rotated
        );
        assert_eq!(s.error_kind(), Some("busy"));
        // 풀리기 전의 받기는 Atlassian을 부르지 않아요.
        let before = gate.calls();
        let (_, info, _, _) = run(&s, d.path());
        assert_eq!(gate.calls(), before);
        assert_eq!(s.error_kind(), Some("busy"));
        assert!(info["lost_since"].is_null());
        assert!(
            !s.changed(d.path(), Duration::from_secs(1))
                .unwrap()
                .unwrap()
        );
        assert_eq!(gate.calls(), before);
    }

    #[test]
    fn products_are_read_once_per_connection() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        let s = source(&gate);
        connect(&s, "good");
        run(&s, d.path());
        run(&s, d.path());
        let count = || {
            gate.fake
                .calls()
                .iter()
                .filter(|c| c.ends_with("accessible-resources"))
                .count()
        };
        assert_eq!(count(), 1);
        // 토큰만 바뀌면 다시 읽지 않아요.
        connect(&s, "good");
        s.set(
            Some("good".into()),
            Some("cloud-1".into()),
            Some("acme".into()),
            None,
        );
        run(&s, d.path());
        assert_eq!(count(), 1);
    }

    #[test]
    fn a_site_without_products_is_not_asked_again() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        gate.fake.resources.lock().unwrap().clear();
        let s = source(&gate);
        connect(&s, "good");
        run(&s, d.path());
        run(&s, d.path());
        let asked = gate
            .fake
            .calls()
            .iter()
            .filter(|c| c.ends_with("accessible-resources"))
            .count();
        assert_eq!(asked, 1);
    }

    #[test]
    fn a_jira_404_turns_jira_off() {
        let d = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeAtlassian::new());
        /// Jira 길이 모두 404인 사이트예요.
        struct NoJira(Arc<FakeAtlassian>);
        impl AtlassianApi for NoJira {
            fn get(&self, path: &str, q: &[(&str, String)]) -> Result<Got, SourceError> {
                if path.contains("/rest/api/3/") {
                    return Ok(Got::Fail {
                        status: 404,
                        reason: String::new(),
                    });
                }
                self.0.get(path, q)
            }
            fn post(&self, path: &str, body: &Value) -> Result<Answer, SourceError> {
                self.0.post(path, body)
            }
        }
        let s = AtlassianSource::with_api(Arc::new(NoJira(fake)));
        connect(&s, "good");
        let (_, info, _, w) = run(&s, d.path());
        assert!(w.is_empty());
        assert!(info["error_kind"].is_null());
        assert!(!state::load_state(d.path()).products.jira);
    }

    #[test]
    fn confluence_pages_get_author_and_space_names_from_jira() {
        let d = tempfile::tempdir().unwrap();
        let gate = Gate::new();
        *gate.fake.resources.lock().unwrap() = vec![json!({
            "id": "cloud-1", "url": "https://acme.atlassian.net", "name": "acme",
            "scopes": ["read:jira-work", "read:page:confluence"]
        })];
        let t = (Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let value = json!({"type": "doc", "version": 1, "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "page body"}]}]})
        .to_string();
        *gate.fake.pages.lock().unwrap() = vec![json!({
            "id": "7", "status": "current", "title": "Auth design", "spaceId": "9",
            "authorId": "u-author", "version": {"number": 1, "createdAt": t, "authorId": "u-author"},
            "body": {"atlas_doc_format": {"value": value}},
            "_links": {"webui": "/spaces/ENG/pages/7"}
        })];
        *gate.fake.users.lock().unwrap() =
            vec![json!({"accountId": "u-author", "displayName": "Ann"})];
        *gate.fake.spaces.lock().unwrap() =
            vec![json!({"id": "9", "key": "ENG", "name": "Engineering"})];
        let s = source(&gate);
        connect(&s, "good");
        let (chunks, info, _, _) = run(&s, d.path());
        assert_eq!(info["pages"], 1);
        let page = chunks.iter().find(|c| c["src"] == "confluence").unwrap();
        assert!(
            page["text"]
                .as_str()
                .unwrap()
                .starts_with("confluence: Auth design\nmeta: Confluence · Engineering · Ann · "),
            "{}",
            page["text"]
        );
        let v = scopes(d.path());
        assert_eq!(v["spaces"][0]["name"], "Engineering");
        assert_eq!(v["projects"][0]["key"], "ENG");
        // 끈 스페이스는 조각에서 빠져요.
        assert!(set_muted(d.path(), &[], &["ENG".into()]).unwrap());
        let chunks = chunks_of(d.path(), Utc::now()).unwrap();
        assert!(chunks.iter().all(|c| c["src"] != "confluence"));
    }

    #[test]
    fn a_new_cloud_is_a_switch() {
        let s = AtlassianSource::default();
        assert_eq!(connect(&s, "a"), Change::Connected);
        let g = s.generation();
        assert_eq!(connect(&s, "b"), Change::Rotated);
        assert_eq!(s.generation(), g);
        assert_eq!(connect(&s, "b"), Change::Same);
        assert_eq!(
            s.set(Some("b".into()), Some("cloud-2".into()), None, None),
            Change::Switched
        );
        assert!(s.generation() > g);
        assert_eq!(s.cloud().as_deref(), Some("cloud-2"));
        assert_eq!(s.set(None, None, None, None), Change::Disconnected);
    }

    #[test]
    fn mutes_and_forget_work_on_disk() {
        let d = tempfile::tempdir().unwrap();
        assert!(set_muted(d.path(), &["P".into(), "P".into()], &[]).unwrap());
        assert!(!set_muted(d.path(), &["P".into()], &[]).unwrap());
        assert!(set_muted(d.path(), &["P".into()], &["S".into()]).unwrap());
        forget(d.path()).unwrap();
        assert!(!d.path().exists());
        forget(d.path()).unwrap();
    }

    #[test]
    fn debug_never_shows_the_token() {
        let s = AtlassianSource::default();
        connect(&s, "secret_token");
        assert!(!format!("{s:?}").contains("secret"));
    }
}
