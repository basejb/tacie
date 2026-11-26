//! Asana raw state (`state.json`) and item files (`items/task-<gid>.json`, `items/project-<gid>.json`) (docs/32 6절).
//!
//! - A task file is the fields Asana returned, as is, plus `workspace` (gid), `parents` (`{gid, name}` from the top
//!   parent down to the direct parent) and `stories` (`comment` only, in time order).
//! - There is one cursor per project (`Proj.cursor`). It moves to the first page's response `Date` − 15 minutes only
//!   when that project's list was fetched to the end. All times are RFC 3339 strings.
//! - One round (`round`): workspace and project lists → task lists → comments → subtasks → mention names. When it
//!   stops on the budget, the next run continues the same round. Projects already fully fetched in this round
//!   (`listed_at >= round`) are not fetched again.
//! - Values the app changes during a fetch (muted workspaces and projects, `lost_since`) and values owned by reconcile
//!   are taken from disk when saving.
//! - When `still()` is false (the connection changed) it writes no more files and stops with `Busy`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::asana::api::{AsanaApi, Reply};
use crate::error::{Kind, SourceError};
use crate::gdrive::fetch::{Budget, safe};
use crate::store::{Stored, read_json, write_json};

pub(crate) const STATE: &str = "state.json";
pub(crate) const ITEMS: &str = "items";
/// 받는 범위예요(지난 2년).
pub(crate) const WINDOW_DAYS: i64 = 730;
/// 한 번에 보내는 요청 수예요(시간은 `gdrive::BATCH_TIME`, 10분).
pub(crate) const BUDGET_REQUESTS: usize = 3_000;

/// 워크스페이스 하나예요.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Ws {
    pub gid: String,
    #[serde(default)]
    pub name: String,
    /// 관리자가 막았어요(`/projects`가 403). 원본을 지웠고 받지 않아요.
    #[serde(default)]
    pub blocked: bool,
    /// `assignee=me` 목록의 커서예요(프로젝트 커서와 같은 규칙).
    #[serde(default)]
    pub me_cursor: Option<String>,
    /// `assignee=me` 목록을 끝까지 받은 Mac 시각이에요.
    #[serde(default)]
    pub me_listed_at: Option<String>,
}

/// 프로젝트 하나예요.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Proj {
    #[serde(default)]
    pub ws: String,
    #[serde(default)]
    pub name: String,
    /// 바뀐 것을 받는 기준이에요. None이면 아직 처음 받기를 끝내지 않았어요.
    #[serde(default)]
    pub cursor: Option<String>,
    /// 목록을 끝까지 받은 Mac 시각이에요.
    #[serde(default)]
    pub listed_at: Option<String>,
    #[serde(default)]
    pub archived: bool,
    /// 이 프로젝트에서 본 가장 늦은 task `modified_at`이에요(30분 확인이 써요).
    #[serde(default)]
    pub changed_at: Option<String>,
}

/// `state.json`이에요.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub version: u32,
    /// 이 원본을 받은 Asana 사용자예요. 다르면 쓰지 않아요.
    #[serde(default)]
    pub user_gid: Option<String>,
    #[serde(default)]
    pub workspaces: Vec<Ws>,
    #[serde(default)]
    pub projects: BTreeMap<String, Proj>,
    /// 댓글을 받을 task예요(바뀐 task 모두). 받을 때 `modified_at` 내림차순으로 늘어놓아요.
    #[serde(default)]
    pub pending_comments: BTreeSet<String>,
    /// 하위 task 목록을 받을 task예요(`num_subtasks > 0`인 바뀐 task).
    #[serde(default)]
    pub pending_subtasks: BTreeSet<String>,
    /// 멘션에 나왔지만 이름을 모르는 사용자예요: gid → 그 멘션이 나온 워크스페이스(끈 워크스페이스면 묻지 않아요).
    #[serde(default)]
    pub pending_users: BTreeMap<String, String>,
    #[serde(default)]
    pub muted_workspaces: Vec<String>,
    #[serde(default)]
    pub muted_projects: Vec<String>,
    /// 사용자 gid → 이름이에요. 못 받은 사람은 빈 글자예요(다시 묻지 않아요).
    #[serde(default)]
    pub users: BTreeMap<String, String>,
    #[serde(default)]
    pub lost_since: Option<String>,
    #[serde(default)]
    pub reconciled_at: Option<String>,
    #[serde(default)]
    pub reconcile_next: Option<String>,
    /// 재조정에서 연달아 빠진 횟수예요: 글 파일 이름(`task-<gid>`, `project-<gid>`) → 횟수.
    #[serde(default)]
    pub gone: BTreeMap<String, u8>,
    /// 재조정의 목록 단계에서 본 task gid예요. 재조정을 끝내면 비워요.
    #[serde(default)]
    pub reconcile_seen: BTreeSet<String>,
    /// 그중 부모의 하위 task 목록이 아니라 프로젝트·`assignee=me` 목록에 나왔거나 `/batch`로 남기로 한 gid예요. 부모를
    /// 지울 때 이 하위 task는 따라 지우지 않아요. 재조정을 끝내면 비워요.
    #[serde(default)]
    pub reconcile_direct: BTreeSet<String>,
    /// 30분 확인이 다음에 물을 프로젝트 자리예요. 30분 확인만 써요.
    #[serde(default)]
    pub probe_next: usize,
    /// 30분 확인이 다음에 물을 최근 프로젝트(30일 안에 바뀐 것) 자리예요. 30분 확인만 써요.
    #[serde(default)]
    pub hot_next: usize,
    #[serde(default)]
    pub fetched_at: Option<String>,
    /// 처음 받기를 한 바퀴 끝냈어요.
    #[serde(default)]
    pub initial_done: bool,
    /// 지금 바퀴를 시작한 Mac 시각이에요. 바퀴를 끝내면 비워요.
    #[serde(default)]
    pub round: Option<String>,
    /// 지금 바퀴의 워크스페이스·프로젝트 목록을 다 받았어요.
    #[serde(default)]
    pub round_listed: bool,
}

impl State {
    /// 받는 워크스페이스예요: 끄지 않았고 막히지 않았어요.
    pub(crate) fn open_workspaces(&self) -> Vec<String> {
        self.workspaces
            .iter()
            .filter(|w| !w.blocked && !self.muted_workspaces.contains(&w.gid))
            .map(|w| w.gid.clone())
            .collect()
    }
}

/// `state.json`, `items/`와 지우기를 함께 막는 자물쇠예요. 엔진에 Asana 원본은 하나라 프로세스에 하나예요.
static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn state_lock() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn load_state(dir: &Path) -> State {
    match read_json(&dir.join(STATE)) {
        Ok(Stored::Value(v)) => serde_json::from_value(v).unwrap_or_default(),
        _ => State::default(),
    }
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

/// 글 파일 자리예요: `items/<kind>-<gid>.json`(`kind`는 `task`나 `project`).
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
            "asana: an item id is not a plain id",
        ));
    }
    fs::create_dir_all(dir.join(ITEMS))?;
    write_json(&item_path(dir, kind, id), v, None)?;
    Ok(())
}

/// 글 파일을 지워요. 없으면 그대로예요.
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

/// 한 걸음의 결과예요. `Later`는 아직 남았어요(예산).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Done,
    Later,
}

pub(crate) fn iso(t: DateTime<Utc>) -> String {
    crate::gdrive::fetch::iso(t)
}

pub(crate) fn parse(s: &str) -> Option<DateTime<Utc>> {
    crate::gdrive::fetch::parse(s)
}

/// 받기 한 번이에요.
pub(crate) struct Run<'a> {
    pub api: &'a dyn AsanaApi,
    pub dir: &'a Path,
    pub user_gid: &'a str,
    pub now: DateTime<Utc>,
    pub still: &'a dyn Fn() -> bool,
    pub state: State,
    pub progress: &'a mut dyn FnMut(usize, usize),
    /// 이번 실행의 몫이에요: 요청 하나마다 하나를 써요.
    pub budget: Budget,
}

impl Run<'_> {
    pub(crate) fn guard(&self) -> Result<(), SourceError> {
        if (self.still)() {
            Ok(())
        } else {
            Err(SourceError::service(
                Kind::Busy,
                "asana: connection changed",
            ))
        }
    }

    /// 요청 하나를 보내요. 몫이 다 찼으면 보내지 않고 None이에요.
    pub(crate) fn get(
        &mut self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Option<Reply>, SourceError> {
        if self.budget.spent() {
            return Ok(None);
        }
        self.guard()?;
        self.budget.take();
        self.api.get(path, query).map(Some)
    }

    pub(crate) fn write(&self, kind: &str, id: &str, v: &Value) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        write_item(self.dir, kind, id, v)
    }

    pub(crate) fn remove(&self, kind: &str, id: &str) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        remove_item(self.dir, kind, id)
    }

    /// 앱이 디스크에 적은 끈 워크스페이스·프로젝트를 다시 읽어요(받는 도중에 바뀔 수 있어요).
    pub(crate) fn refresh_mutes(&mut self) {
        let disk = {
            let _held = state_lock();
            load_state(self.dir)
        };
        self.state.muted_workspaces = disk.muted_workspaces;
        self.state.muted_projects = disk.muted_projects;
    }

    /// 상태를 써요. 받기가 맡지 않은 값(끈 워크스페이스·프로젝트, `lost_since`, 재조정의 값, 30분 확인 자리)은 디스크
    /// 것을 그대로 둬요. 디스크의 사용자가 지금 사용자와 다르면 쓰지 않아요.
    pub(crate) fn save(&mut self) -> Result<(), SourceError> {
        self.save_with(false)
    }

    /// 재조정이 상태를 써요. `save`와 같지만 재조정이 맡은 값은 이 실행의 것을 써요.
    pub(crate) fn save_upkeep(&mut self) -> Result<(), SourceError> {
        self.save_with(true)
    }

    fn save_with(&mut self, upkeep: bool) -> Result<(), SourceError> {
        let _held = state_lock();
        self.guard()?;
        let disk = load_state(self.dir);
        if disk.user_gid.as_deref().is_some_and(|u| u != self.user_gid) {
            return Err(SourceError::service(
                Kind::Busy,
                "asana: the saved items are another user's",
            ));
        }
        self.state.user_gid = Some(self.user_gid.to_string());
        self.state.muted_workspaces = disk.muted_workspaces;
        self.state.muted_projects = disk.muted_projects;
        self.state.lost_since = disk.lost_since;
        self.state.probe_next = disk.probe_next;
        self.state.hot_next = disk.hot_next;
        if !upkeep {
            self.state.reconciled_at = disk.reconciled_at;
            self.state.reconcile_next = disk.reconcile_next;
            self.state.gone = disk.gone;
            self.state.reconcile_seen = disk.reconcile_seen;
            self.state.reconcile_direct = disk.reconcile_direct;
        }
        save_state(self.dir, &self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips_and_items_are_listed_by_kind() {
        let d = tempfile::tempdir().unwrap();
        let mut s = State::default();
        s.projects.insert(
            "p1".into(),
            Proj {
                ws: "w1".into(),
                cursor: Some("2026-11-12T00:00:00.000Z".into()),
                ..Proj::default()
            },
        );
        s.pending_comments.insert("7".into());
        s.muted_projects = vec!["p2".into()];
        save_state(d.path(), &s).unwrap();
        let back = load_state(d.path());
        assert_eq!(back.version, 1);
        assert_eq!(back.projects, s.projects);
        assert_eq!(back.pending_comments, s.pending_comments);
        assert_eq!(back.muted_projects, ["p2"]);

        write_item(d.path(), "task", "10", &serde_json::json!({"gid": "10"})).unwrap();
        write_item(d.path(), "project", "7", &serde_json::json!({"gid": "7"})).unwrap();
        assert_eq!(item_ids(d.path(), "task"), ["10"]);
        assert_eq!(item_ids(d.path(), "project"), ["7"]);
        assert_eq!(read_item(d.path(), "task", "10").unwrap()["gid"], "10");
        assert!(read_item(d.path(), "task", "../x").is_none());
        assert!(write_item(d.path(), "task", "../x", &Value::Null).is_err());
        remove_item(d.path(), "task", "10").unwrap();
        assert!(item_ids(d.path(), "task").is_empty());
    }
}
