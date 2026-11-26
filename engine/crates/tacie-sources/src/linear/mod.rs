//! Linear (docs/18): fetches issues, comments, project updates and documents from the last 2 years in teams the
//! connected person can see.

pub mod api;
pub mod docs;
pub mod fetch;

use std::path::Path;

use crate::error::SourceError;

pub use docs::{LOST_DAYS, TeamInfo, chunks_of, teams};

/// 끈 팀을 `state.json`에 적어요. 바뀌었으면 참이에요.
pub fn set_muted(raw_dir: &Path, muted: &[String]) -> Result<bool, SourceError> {
    let _held = fetch::state_lock();
    let mut s = fetch::load_state(raw_dir);
    let mut new: Vec<String> = muted.to_vec();
    new.sort();
    new.dedup();
    if s.muted == new {
        return Ok(false);
    }
    s.muted = new;
    fetch::save_state(raw_dir, &s)?;
    Ok(true)
}

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use tacie_index::RawChunk;

use crate::error::Kind;
use api::{LinearApi, LinearHttp};

pub use api::API;

/// 앱이 연결을 넘길 때 무엇이 바뀌었는지예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// 같아요.
    Same,
    /// 토큰만 바뀌었어요(하루마다 갱신). 동기화를 걸지 않아요.
    Rotated,
    /// 처음 연결했어요.
    Connected,
    /// 다른 워크스페이스예요. 받아 둔 것을 지우고 처음부터 받아요.
    Switched,
    /// 연결을 끊었어요.
    Disconnected,
}

#[derive(Clone, PartialEq, Eq)]
struct Conn {
    token: String,
    workspace: Option<String>,
    workspace_id: Option<String>,
}

/// Linear 받기예요(갱신이 불러요).
pub trait LinearFetch: Send + Sync {
    fn connected(&self) -> bool;
    /// 연결의 세대예요. 워크스페이스를 바꾸거나 끊을 때마다 늘어요.
    fn generation(&self) -> u64;
    /// `raw` 아래를 새로 받아요. 받는 중 세대가 바뀌면 멈춰요.
    fn fetch(&self, raw: &Path, progress: &mut dyn FnMut(usize, usize)) -> Result<(), SourceError>;
    fn workspace(&self) -> Option<String>;
}

/// 진짜 Linear예요: 앱이 넘긴 토큰(메모리에만)으로 `LinearHttp`를 불러요. 테스트는 `api`에 가짜를 넣어요.
#[derive(Default)]
pub struct LinearSource {
    pub base: String,
    conn: RwLock<Option<Conn>>,
    generation: AtomicU64,
    refused: AtomicBool,
    /// 지난 받기가 실패한 종류예요(`/index`의 `linear.error_kind`). 성공하면 비워요.
    error: std::sync::Mutex<Option<Kind>>,
    /// 거절 뒤 앱이 갱신한 토큰으로 다시 받기를 건 때예요. 30분에 한 번만 걸어요(또 거절되면 30분 확인에 맡겨요).
    retried: std::sync::Mutex<Option<std::time::Instant>>,
    api: Option<Arc<dyn LinearApi>>,
}

impl fmt::Debug for LinearSource {
    /// 주소와 연결 여부만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinearSource")
            .field("base", &self.base)
            .field("connected", &self.connected())
            .finish_non_exhaustive()
    }
}

impl LinearSource {
    pub fn new(base: &str) -> Self {
        LinearSource {
            base: base.to_string(),
            ..Default::default()
        }
    }

    /// 진짜 API 대신 `api`를 부르는 Linear예요(테스트).
    pub fn with_api(api: Arc<dyn LinearApi>) -> Self {
        LinearSource {
            api: Some(api),
            ..Default::default()
        }
    }

    /// 연결을 바꿔요. 토큰만 바뀐 것은 `Rotated`이고 세대를 올리지 않아요.
    pub fn set(
        &self,
        token: Option<String>,
        workspace: Option<String>,
        workspace_id: Option<String>,
    ) -> Change {
        let new = token.map(|token| Conn {
            token,
            workspace,
            workspace_id,
        });
        let mut cur = self.conn.write().unwrap_or_else(PoisonError::into_inner);
        let change = match (&*cur, &new) {
            (None, None) => Change::Same,
            (None, Some(_)) => Change::Connected,
            (Some(_), None) => Change::Disconnected,
            (Some(a), Some(b)) if a.workspace_id != b.workspace_id => Change::Switched,
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

    /// 지난 받기가 실패한 종류예요. 성공했거나 아직 받지 않았으면 None이에요.
    pub fn retry_after_refusal(&self) -> bool {
        let mut at = self.retried.lock().unwrap_or_else(PoisonError::into_inner);
        if at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(30 * 60)) {
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

    pub fn workspace_id(&self) -> Option<String> {
        self.conn
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|c| c.workspace_id.clone())
    }

    /// 30분 확인용 빠른 클라이언트예요(한 번만, 짧게). 연결이 없거나 테스트 가짜면 그 가짜예요.
    pub fn checker(&self, timeout: std::time::Duration) -> Option<Arc<dyn LinearApi>> {
        let token = self
            .conn
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()?
            .token
            .clone();
        Some(match &self.api {
            Some(api) => api.clone(),
            None => Arc::new(LinearHttp::with_tries(&self.base, &token, 1, timeout)),
        })
    }
}

impl LinearFetch for LinearSource {
    fn connected(&self) -> bool {
        self.conn
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn workspace(&self) -> Option<String> {
        self.conn
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|c| c.workspace.clone())
    }

    fn fetch(&self, raw: &Path, progress: &mut dyn FnMut(usize, usize)) -> Result<(), SourceError> {
        let Some(conn) = self
            .conn
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        else {
            return Err(SourceError::service(Kind::Key, "linear: not connected"));
        };
        let g = self.generation();
        let still = || self.generation() == g;
        let http;
        let api: &dyn LinearApi = match &self.api {
            Some(api) => api.as_ref(),
            None => {
                http = LinearHttp::new(&self.base, &conn.token);
                &http
            }
        };
        let got = fetch::fetch_with(
            api,
            raw,
            Utc::now(),
            conn.workspace_id.as_deref(),
            &still,
            progress,
        );
        if matches!(&got, Err(e) if e.kind() == Kind::Key) {
            self.refused.store(true, Ordering::SeqCst);
        }
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) =
            got.as_ref().err().map(SourceError::kind);
        if got.is_ok() && still() {
            let _held = fetch::state_lock();
            let mut s = fetch::load_state(raw);
            s.workspace_id = conn.workspace_id;
            s.workspace = conn.workspace;
            fetch::save_state(raw, &s)?;
        }
        got
    }
}

fn saved(raw_dir: &Path) -> bool {
    raw_dir.join(fetch::STATE).exists() && raw_dir.join(fetch::ITEMS).exists()
}

/// 갱신의 Linear 몫이에요: 연결돼 있으면(그리고 `fetch`거나 저장본이 없으면) 받고, 저장본의 조각과 info
/// (`{workspace, fetched_at, issues, teams, chunks, lost_since, error_kind}`)를 돌려줘요. 저장본이 없으면 둘 다
/// 비어요. 받지 못했는데 저장본이 있으면 경고 `linear`를 남기고 저장본으로 가요. 거절(`key`)이면 `lost_since`를
/// 적고, 받기에 성공하면 지워요.
pub fn gather(
    source: &dyn LinearFetch,
    raw_dir: &Path,
    fetch: bool,
    progress: &mut dyn FnMut(usize, usize),
    warnings: &mut Vec<String>,
) -> Result<(Vec<RawChunk>, Option<Value>), SourceError> {
    let mut failed: Option<Kind> = None;
    if source.connected() && (fetch || !saved(raw_dir)) {
        let got = crate::gather::fetch_or_saved(
            "linear",
            || {
                let r = source.fetch(raw_dir, progress);
                if let Err(e) = &r {
                    failed = Some(e.kind());
                }
                r
            },
            || saved(raw_dir),
            warnings,
        );
        let held = fetch::state_lock();
        let mut s = fetch::load_state(raw_dir);
        match failed {
            Some(Kind::Key) if s.lost_since.is_none() && saved(raw_dir) => {
                s.lost_since = Some(fetch::iso(Utc::now()));
                fetch::save_state(raw_dir, &s)?;
            }
            None if s.lost_since.is_some() => {
                s.lost_since = None;
                fetch::save_state(raw_dir, &s)?;
            }
            _ => {}
        }
        drop(held);
        got?;
    }
    if !saved(raw_dir) {
        return Ok((Vec::new(), None));
    }
    let s = fetch::load_state(raw_dir);
    let chunks = chunks_of(raw_dir, Utc::now())?;
    let issues = fetch::item_names(raw_dir)
        .iter()
        .filter(|n| n.starts_with("issue-"))
        .count();
    let mut info = Map::new();
    info.insert(
        "workspace".into(),
        source
            .workspace()
            .or(s.workspace.clone())
            .map_or(Value::Null, Value::from),
    );
    info.insert(
        "fetched_at".into(),
        s.fetched_at.clone().map_or(Value::Null, Value::from),
    );
    info.insert("issues".into(), Value::from(issues));
    info.insert("teams".into(), Value::from(s.teams.len()));
    info.insert("chunks".into(), Value::from(chunks.len()));
    info.insert(
        "lost_since".into(),
        s.lost_since.clone().map_or(Value::Null, Value::from),
    );
    info.insert(
        "error_kind".into(),
        failed.map_or(Value::Null, |k| Value::from(k.as_str())),
    );
    Ok((chunks, Some(Value::Object(info))))
}

/// 받은 지 `secs`초가 지났거나 받은 적이 없는지예요.
pub fn due(raw_dir: &Path, secs: i64) -> bool {
    let last = fetch::load_state(raw_dir)
        .fetched_at
        .and_then(|f| DateTime::parse_from_rfc3339(&f).ok());
    last.is_none_or(|l| Utc::now().timestamp() - l.timestamp() >= secs)
}

/// 저장본에서 가장 늦은 커서예요(30분 확인이 Linear의 최신 항목과 견줘요).
pub fn latest(raw_dir: &Path) -> Option<String> {
    fetch::load_state(raw_dir).cursors.values().max().cloned()
}

/// 저장본의 워크스페이스 id예요.
pub fn stored_workspace(raw_dir: &Path) -> Option<String> {
    fetch::load_state(raw_dir).workspace_id
}

/// 받은 Linear 자료를 모두 지워요(연결 해제, 다른 워크스페이스).
pub fn forget(raw_dir: &Path) -> std::io::Result<()> {
    let _held = fetch::state_lock();
    match std::fs::remove_dir_all(raw_dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linear::api::{LinearApi, Reply};
    use serde_json::{Value, json};
    use std::sync::Arc;

    struct Answers(Value);
    impl LinearApi for Answers {
        fn query(&self, q: &str, _: &Value) -> Result<Reply, SourceError> {
            if self.0.is_null() {
                return Err(SourceError::service(crate::Kind::Key, "linear: refused"));
            }
            let root = ["teams", "issues", "documents", "projectUpdates", "comments"]
                .into_iter()
                .find(|r| q.contains(&format!("{{ {r}(")))
                .unwrap();
            let nodes = if root == "teams" {
                json!([{"id": "t1", "name": "Core", "key": "C"}])
            } else if root == "issues" {
                self.0.clone()
            } else {
                json!([])
            };
            Ok(Reply {
                data: Some(json!({root: {"nodes": nodes, "pageInfo": {"hasNextPage": false}}})),
                errors: vec![],
            })
        }
    }
    fn issue() -> Value {
        json!([{"id": "1", "identifier": "C-1", "title": "T", "url": "u", "state": {"type": "started"}, "team": {"id": "t1", "name": "Core"},
                "updatedAt": "2026-10-30T00:00:00.000Z", "trashed": false}])
    }

    #[test]
    fn a_rotated_token_is_not_a_change() {
        let s = LinearSource::default();
        assert_eq!(
            s.set(Some("a".into()), Some("W".into()), Some("w1".into())),
            Change::Connected
        );
        let g = s.generation();
        assert_eq!(
            s.set(Some("b".into()), Some("W".into()), Some("w1".into())),
            Change::Rotated
        );
        assert_eq!(
            s.set(Some("b".into()), Some("W".into()), Some("w1".into())),
            Change::Same
        );
        assert_eq!(s.generation(), g);
    }

    #[test]
    fn a_new_workspace_bumps_the_generation() {
        let s = LinearSource::default();
        s.set(Some("a".into()), None, Some("w1".into()));
        let g = s.generation();
        assert_eq!(
            s.set(Some("a".into()), None, Some("w2".into())),
            Change::Switched
        );
        assert!(s.generation() > g);
        let g = s.generation();
        assert_eq!(s.set(None, None, None), Change::Disconnected);
        assert!(s.generation() > g);
    }

    #[test]
    fn a_refusal_marks_refused_and_lost_since_and_a_success_clears_them() {
        let d = tempfile::tempdir().unwrap();
        let ok = LinearSource::with_api(Arc::new(Answers(issue())));
        ok.set(Some("a".into()), Some("W".into()), Some("w1".into()));
        let mut w = Vec::new();
        let (chunks, info) = gather(&ok, d.path(), true, &mut |_, _| {}, &mut w).unwrap();
        assert!(!chunks.is_empty());
        let info = info.unwrap();
        assert_eq!(info["workspace"], "W");
        assert_eq!(info["issues"], 1);
        assert_eq!(stored_workspace(d.path()).as_deref(), Some("w1"));

        let bad = LinearSource::with_api(Arc::new(Answers(Value::Null)));
        bad.set(Some("a".into()), Some("W".into()), Some("w1".into()));
        let (chunks, info) = gather(&bad, d.path(), true, &mut |_, _| {}, &mut w).unwrap();
        assert!(bad.refused());
        assert!(!chunks.is_empty(), "the saved copy is still searched");
        let info = info.unwrap();
        assert_eq!(info["error_kind"], "key");
        assert!(info["lost_since"].is_string());

        gather(&ok, d.path(), true, &mut |_, _| {}, &mut w).unwrap();
        assert!(fetch::load_state(d.path()).lost_since.is_none());
    }

    #[test]
    fn a_first_refusal_without_a_saved_copy_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let bad = LinearSource::with_api(Arc::new(Answers(Value::Null)));
        bad.set(Some("a".into()), None, Some("w1".into()));
        let mut w = Vec::new();
        assert_eq!(
            gather(&bad, d.path(), true, &mut |_, _| {}, &mut w)
                .unwrap_err()
                .kind(),
            crate::Kind::Key
        );
    }

    #[test]
    fn without_a_connection_the_saved_copy_is_still_searched_until_forgotten() {
        let d = tempfile::tempdir().unwrap();
        let ok = LinearSource::with_api(Arc::new(Answers(issue())));
        ok.set(Some("a".into()), None, Some("w1".into()));
        let mut w = Vec::new();
        gather(&ok, d.path(), true, &mut |_, _| {}, &mut w).unwrap();
        let off = LinearSource::default();
        assert!(
            !gather(&off, d.path(), true, &mut |_, _| {}, &mut w)
                .unwrap()
                .0
                .is_empty()
        );
        forget(d.path()).unwrap();
        assert!(
            gather(&off, d.path(), true, &mut |_, _| {}, &mut w)
                .unwrap()
                .1
                .is_none()
        );
    }

    #[test]
    fn mutes_are_read_from_disk_without_a_connection() {
        let d = tempfile::tempdir().unwrap();
        let ok = LinearSource::with_api(Arc::new(Answers(issue())));
        ok.set(Some("a".into()), None, Some("w1".into()));
        let mut w = Vec::new();
        gather(&ok, d.path(), true, &mut |_, _| {}, &mut w).unwrap();
        set_muted(d.path(), &["t1".to_string()]).unwrap();
        let off = LinearSource::default();
        assert!(
            gather(&off, d.path(), false, &mut |_, _| {}, &mut w)
                .unwrap()
                .0
                .is_empty()
        );
    }

    #[test]
    fn debug_never_shows_the_token() {
        let s = LinearSource::default();
        s.set(Some("lin_secret_token".into()), None, None);
        assert!(!format!("{s:?}").contains("secret"));
    }

    #[test]
    fn due_and_latest_follow_the_saved_fetch() {
        let d = tempfile::tempdir().unwrap();
        assert!(due(d.path(), 900));
        assert!(latest(d.path()).is_none());
        let ok = LinearSource::with_api(Arc::new(Answers(issue())));
        ok.set(Some("a".into()), None, Some("w1".into()));
        gather(&ok, d.path(), true, &mut |_, _| {}, &mut Vec::new()).unwrap();
        assert!(!due(d.path(), 900));
        assert!(latest(d.path()).is_some());
    }
}
