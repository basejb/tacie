//! Atlassian raw state (`state.json`) and item files (`items/<종류>-<id>.json`). Jira and Confluence share them.
//!
//! - There is one cursor (`Cursor`) per list: Jira, pages, blog posts, footer comments, inline comments. `head` is the
//!   reference for change fetches (the latest modified time fetched so far), `floor` is the oldest modified time the
//!   initial fetch has gone down to, and `floor_key` is the last item seen within that minute (Jira key). Jira times
//!   are kept exactly as the strings Jira returns (`2026-11-12T10:30:00.000+0900`). The zone offset stays in the
//!   string and is used to build JQL times.
//! - Values the app changes during a fetch (muted projects and spaces, `lost_since`) are kept as they are on disk when
//!   the fetch saves.
//! - When `still()` is false (the connection changed) it writes no more files and stops with `Busy`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::atlassian::api::{AtlassianApi, Points};
use crate::error::{Kind, SourceError};
use crate::gdrive::api::Got;
use crate::gdrive::fetch::safe;
use crate::store::{Stored, read_json, write_json};

pub(crate) const STATE: &str = "state.json";
pub(crate) const ITEMS: &str = "items";
/// 처음 받기가 내려가는 끝이에요(지난 2년).
pub(crate) const WINDOW_DAYS: i64 = 730;

/// 이 사이트에 있는 제품이에요(`accessible-resources`의 scope로 정해요).
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Products {
    #[serde(default)]
    pub jira: bool,
    #[serde(default)]
    pub confluence: bool,
}

/// 목록 하나의 커서예요.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Cursor {
    /// 지금까지 받은 가장 늦은 수정 시각이에요. 바뀐 것은 이보다 조금 앞부터 받아요.
    #[serde(default)]
    pub head: Option<String>,
    /// 처음 받기가 내려간 가장 오래된 수정 시각이에요.
    #[serde(default)]
    pub floor: Option<String>,
    /// `floor`의 분 안에서 마지막으로 본 글이에요(Jira key). 그 분을 이어 받을 때 여기서 갈라요.
    #[serde(default)]
    pub floor_key: Option<String>,
    /// 처음 받기가 다음에 이을 목록 커서예요(Confluence `cursor=`). 목록을 끝내면 비워요.
    #[serde(default)]
    pub next: Option<String>,
    /// 처음 받기를 끝냈는지예요(2년 끝에 닿았어요).
    #[serde(default)]
    pub done: bool,
}

/// `state.json`이에요.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub cloud: Option<String>,
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub site_url: Option<String>,
    #[serde(default)]
    pub products: Products,
    #[serde(default)]
    pub jira: Cursor,
    #[serde(default)]
    pub pages: Cursor,
    #[serde(default)]
    pub blogs: Cursor,
    #[serde(default)]
    pub footer: Cursor,
    #[serde(default)]
    pub inline: Cursor,
    #[serde(default)]
    pub muted_projects: Vec<String>,
    #[serde(default)]
    pub muted_spaces: Vec<String>,
    /// 처음 받기의 시간당 포인트 장부예요.
    #[serde(default)]
    pub points: Points,
    /// 시간당 한도로 멈춘 때까지예요(RFC 3339). 이때까지 Atlassian을 부르지 않아요.
    #[serde(default)]
    pub paused_until: Option<String>,
    #[serde(default)]
    pub lost_since: Option<String>,
    #[serde(default)]
    pub reconciled_at: Option<String>,
    #[serde(default)]
    pub reported_at: Option<String>,
    /// 개인정보 보고 간격(초, `Cycle-Period`)이에요.
    #[serde(default)]
    pub report_every: Option<u64>,
    /// 재조정에서 연달아 빠진 횟수예요: 글 파일 이름(`jira-<id>`, `page-<id>`) → 횟수.
    #[serde(default)]
    pub gone: BTreeMap<String, u8>,
    /// 예산으로 멈춘 재조정이 다음에 이을 자리예요: `<단계>:<마지막으로 끝낸 id>`(예: `jira:10`, `comments:7`).
    /// 재조정을 다 끝내면 비워요.
    #[serde(default)]
    pub reconcile_next: Option<String>,
    /// 개인정보 보고에서 탈퇴(`closed`)로 온 accountId예요. 받기·재조정이 글 파일을 쓸 때마다 이 사람들을 지워서
    /// 다시 받아도 돌아오지 않아요.
    #[serde(default)]
    pub closed: BTreeSet<String>,
    #[serde(default)]
    pub fetched_at: Option<String>,
    /// Jira `myself.timeZone`이에요(예: `Asia/Seoul`).
    #[serde(default)]
    pub time_zone: Option<String>,
    /// Confluence 글쓴이·댓글 작성자의 이름이에요: accountId → 이름. Confluence v2는 `authorId`만 줘서 Jira
    /// `user/bulk`로 받아 둬요(Jira가 있는 사이트만). 원본에 나오는 id만 남겨요.
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    /// Confluence 스페이스 이름이에요: spaceId → (key, 이름). 원본에 나오는 스페이스만 남겨요.
    #[serde(default)]
    pub spaces: BTreeMap<String, SpaceName>,
}

/// 스페이스 하나의 key와 이름이에요.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SpaceName {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub name: String,
}

/// `state.json`, `items/`와 지우기를 함께 막는 자물쇠예요. 엔진에 Atlassian 원본은 하나라 프로세스에 하나예요.
static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn state_lock() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn load_state(dir: &Path) -> State {
    let mut s: State = match read_json(&dir.join(STATE)) {
        Ok(Stored::Value(v)) => serde_json::from_value(v).unwrap_or_default(),
        _ => State::default(),
    };
    // 하루(+ 흔들기 15분)보다 먼 멈춤은 잘못 적힌 값이라 보지 않아요(`api::MAX_PAUSE_HOURS`).
    let limit = Utc::now()
        + chrono::Duration::hours(crate::atlassian::api::MAX_PAUSE_HOURS)
        + chrono::Duration::minutes(15);
    if s.paused_until
        .as_deref()
        .and_then(crate::gdrive::fetch::parse)
        .is_some_and(|t| t > limit)
    {
        s.paused_until = None;
    }
    s
}

pub(crate) fn save_state(dir: &Path, s: &State) -> Result<(), SourceError> {
    fs::create_dir_all(dir)?;
    let mut s = s.clone();
    s.version = 1;
    write_json(
        &dir.join(STATE),
        &serde_json::to_value(&s).map_err(|e| SourceError::Json(e.to_string()))?,
        None,
    )?;
    Ok(())
}

/// 글 파일 자리예요: `items/<kind>-<id>.json`(`kind`는 `jira`나 `page`).
pub(crate) fn item_path(dir: &Path, kind: &str, id: &str) -> PathBuf {
    dir.join(ITEMS).join(format!("{kind}-{id}.json"))
}

pub(crate) fn read_item(dir: &Path, kind: &str, id: &str) -> Option<Value> {
    if !safe(id) {
        return None;
    }
    match read_json(&item_path(dir, kind, id)) {
        Ok(Stored::Value(v)) => Some(v),
        _ => None,
    }
}

/// 글 파일을 써요. 자물쇠와 `still` 확인은 부르는 쪽(`Run::write`)이 해요.
pub(crate) fn write_item(dir: &Path, kind: &str, id: &str, v: &Value) -> Result<(), SourceError> {
    if !safe(id) {
        return Err(SourceError::service(
            Kind::Down,
            "atlassian: an item id is not a plain id",
        ));
    }
    fs::create_dir_all(dir.join(ITEMS))?;
    write_json(&item_path(dir, kind, id), v, None)?;
    Ok(())
}

/// 글 파일을 지워요. 없으면 그대로예요. 자물쇠와 `still` 확인은 부르는 쪽(`Run::remove`)이 해요.
pub(crate) fn remove_item(dir: &Path, kind: &str, id: &str) -> Result<(), SourceError> {
    if !safe(id) {
        return Ok(());
    }
    match fs::remove_file(item_path(dir, kind, id)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// `kind`의 저장된 글 id예요(정렬해요).
pub(crate) fn item_ids(dir: &Path, kind: &str) -> Vec<String> {
    let prefix = format!("{kind}-");
    let mut ids: Vec<String> = fs::read_dir(dir.join(ITEMS))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_suffix(".json")?
                .strip_prefix(&prefix)
                .map(str::to_string)
        })
        .collect();
    ids.sort();
    ids
}

/// 받기 한 번의 결과예요. `Later`는 아직 남았어요(예산, 한도).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Done,
    Later,
}

/// 받기 한 번이에요. Jira와 Confluence가 같이 써요.
pub(crate) struct Run<'a> {
    pub api: &'a dyn AtlassianApi,
    pub dir: &'a Path,
    pub cloud: &'a str,
    pub now: DateTime<Utc>,
    pub still: &'a dyn Fn() -> bool,
    pub state: State,
    pub progress: &'a mut dyn FnMut(usize, usize),
    /// 시간당 한도 뒤에 더 늦출 초예요(0–900). 진짜는 `jitter`이고 테스트는 정한 값을 줘요.
    pub jitter: &'a dyn Fn() -> i64,
    /// 한도로 멈췄을 때의 오류예요(`Busy`). 받기는 `Step::Later`로 끝나고, 부르는 쪽이 이걸 보고 알려요.
    pub halted: Option<SourceError>,
}

/// 시간당 한도 뒤에 더 늦출 0–900초예요. 정각에 모든 Mac이 같이 다시 시작하지 않게 흩어요. 난수 crate 없이 지금
/// 시각의 나노초로 만들어요(Mac마다 다르면 충분해요).
pub(crate) fn jitter() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    i64::from(nanos % 901)
}

/// 답 몸통을 JSON으로 읽어요. 실패(`Fail`)나 JSON이 아니면 `Down`이에요(오류 글에 내용은 넣지 않아요).
pub(crate) fn json_of(got: Got, what: &str) -> Result<Value, SourceError> {
    match got {
        Got::Body(b) => serde_json::from_slice(&b).map_err(|_| {
            SourceError::service(Kind::Down, format!("atlassian: {what} is not JSON"))
        }),
        Got::Fail { status, .. } => Err(SourceError::service(
            Kind::Down,
            format!("atlassian: {what} failed {status}"),
        )),
    }
}

pub(crate) fn iso(t: DateTime<Utc>) -> String {
    crate::gdrive::fetch::iso(t)
}

impl Run<'_> {
    pub(crate) fn guard(&self) -> Result<(), SourceError> {
        if (self.still)() {
            Ok(())
        } else {
            Err(SourceError::service(
                Kind::Busy,
                "atlassian: connection changed",
            ))
        }
    }

    /// 글 파일 하나를 써요. 탈퇴한 사람(`state.closed`)은 쓰기 전에 지워요(`privacy::forget`).
    pub(crate) fn write(&self, kind: &str, id: &str, v: &Value) -> Result<(), SourceError> {
        let mut v = v.clone();
        if !self.state.closed.is_empty() {
            for who in crate::atlassian::privacy::ids_of(&v).intersection(&self.state.closed) {
                crate::atlassian::privacy::forget(&mut v, who);
            }
        }
        let _held = state_lock();
        self.guard()?;
        write_item(self.dir, kind, id, &v)
    }

    /// 글 파일 하나를 지워요(Confluence에서 휴지통에 들어가거나 지운 글).
    pub(crate) fn remove(&self, kind: &str, id: &str) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        remove_item(self.dir, kind, id)
    }

    /// 상태를 써요. 받기가 맡지 않은 값은 디스크 것을 그대로 둬요: 앱이 받는 도중에 바꿀 수 있는 값(끈 프로젝트·
    /// 스페이스, `lost_since`)과 연결·재조정·개인정보 보고가 쓰는 값(`products`, `reconciled_at`, `reported_at`,
    /// `report_every`, `gone`, `reconcile_next`, `closed`)이에요. 디스크의 사이트가 지금 사이트와 다르면 쓰지 않아요.
    pub(crate) fn save(&mut self) -> Result<(), SourceError> {
        self.save_with(None, false)
    }

    /// 상태를 쓰면서 제품을 바꿔요. 받기가 이 사이트에 그 제품이 없다고 알았을 때(Confluence 404)만 써요.
    pub(crate) fn save_products(&mut self, products: Products) -> Result<(), SourceError> {
        self.save_with(Some(products), false)
    }

    /// 재조정·개인정보 보고가 상태를 써요. `save`와 같지만 그 둘이 맡은 값(`reconciled_at`, `reported_at`,
    /// `report_every`, `gone`, `reconcile_next`, `closed`)은 디스크 것 대신 이 받기의 것을 써요.
    pub(crate) fn save_upkeep(&mut self) -> Result<(), SourceError> {
        self.save_with(None, true)
    }

    fn save_with(&mut self, products: Option<Products>, upkeep: bool) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        let disk = load_state(self.dir);
        if disk.cloud.as_deref().is_some_and(|c| c != self.cloud) {
            return Err(SourceError::service(
                Kind::Busy,
                "atlassian: the saved items are another site's",
            ));
        }
        self.state.cloud = Some(self.cloud.to_string());
        self.state.muted_projects = disk.muted_projects;
        self.state.muted_spaces = disk.muted_spaces;
        self.state.lost_since = disk.lost_since;
        self.state.products = products.unwrap_or(disk.products);
        if !upkeep {
            self.state.reconciled_at = disk.reconciled_at;
            self.state.reported_at = disk.reported_at;
            self.state.report_every = disk.report_every;
            self.state.gone = disk.gone;
            self.state.reconcile_next = disk.reconcile_next;
            self.state.closed = disk.closed;
        }
        save_state(self.dir, &self.state)
    }

    /// 시간당 한도로 멈췄는지 봐요. 지난 멈춤은 지워요.
    pub(crate) fn paused(&mut self) -> bool {
        let until = self
            .state
            .paused_until
            .as_deref()
            .and_then(crate::gdrive::fetch::parse);
        match until {
            Some(t) if t > self.now => {
                self.halted = Some(SourceError::service(
                    Kind::Busy,
                    "atlassian: paused for the hourly limit",
                ));
                true
            }
            Some(_) => {
                self.state.paused_until = None;
                false
            }
            None => false,
        }
    }

    /// 오류가 시간당 한도면 풀리는 때 + 0–15분까지 멈추고 상태를 저장한 뒤 `Later`예요. 아니면 오류 그대로예요.
    pub(crate) fn pause_on_quota(&mut self, e: SourceError) -> Result<Step, SourceError> {
        let Some(reset) = crate::atlassian::api::quota_until(&e) else {
            return Err(e);
        };
        let wait = (self.jitter)().clamp(0, 900);
        self.state.paused_until = Some(iso(reset + chrono::Duration::seconds(wait)));
        self.halted = Some(e);
        self.save()?;
        Ok(Step::Later)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pause_more_than_a_day_ahead_is_ignored_on_load() {
        let d = tempfile::tempdir().unwrap();
        let mut s = State {
            paused_until: Some("2099-01-01T00:00:00Z".into()),
            ..State::default()
        };
        save_state(d.path(), &s).unwrap();
        assert_eq!(load_state(d.path()).paused_until, None);
        // 한도의 하루 + 흔들기 15분 안이면 그대로예요
        let near = iso(Utc::now() + chrono::Duration::hours(24) + chrono::Duration::minutes(10));
        s.paused_until = Some(near.clone());
        save_state(d.path(), &s).unwrap();
        assert_eq!(load_state(d.path()).paused_until, Some(near));
    }

    #[test]
    fn state_round_trips_and_items_are_listed_by_kind() {
        let d = tempfile::tempdir().unwrap();
        let mut s = State::default();
        s.jira.floor = Some("2026-11-12T10:30:00.000+0900".into());
        s.muted_projects = vec!["ENG".into()];
        s.gone.insert("10".into(), 1);
        save_state(d.path(), &s).unwrap();
        let back = load_state(d.path());
        assert_eq!(back.version, 1);
        assert_eq!(back.jira, s.jira);
        assert_eq!(back.muted_projects, ["ENG"]);
        assert_eq!(back.gone.get("10"), Some(&1));

        write_item(d.path(), "jira", "10", &serde_json::json!({"id": "10"})).unwrap();
        write_item(d.path(), "page", "7", &serde_json::json!({"id": "7"})).unwrap();
        assert_eq!(item_ids(d.path(), "jira"), ["10"]);
        assert_eq!(item_ids(d.path(), "page"), ["7"]);
        assert_eq!(read_item(d.path(), "jira", "10").unwrap()["id"], "10");
        assert!(read_item(d.path(), "jira", "../x").is_none());
        assert!(write_item(d.path(), "jira", "../x", &Value::Null).is_err());
    }

    #[test]
    fn json_errors_say_atlassian() {
        let e = json_of(Got::Body(b"nope".to_vec()), "jira search").unwrap_err();
        assert!(e.to_string().contains("atlassian: jira search"), "{e}");
        let e = json_of(
            Got::Fail {
                status: 404,
                reason: String::new(),
            },
            "x",
        )
        .unwrap_err();
        assert_eq!(e.kind(), Kind::Down);
        assert!(!e.to_string().contains("gdrive"));
    }

    #[test]
    fn jitter_stays_within_fifteen_minutes() {
        for _ in 0..50 {
            assert!((0..=900).contains(&jitter()));
        }
    }
}
