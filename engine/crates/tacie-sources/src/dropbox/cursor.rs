//! Fetches changes with the Dropbox cursor, and checks every 30 minutes whether anything changed (docs/33 6절).
//!
//! - Deleted entries (`deleted`) have no id, so saved files under that path become delete candidates (`doomed`) and
//!   are deleted after the cursor has been read to the end. Renames, folder moves and shared folder re-mounts bring the
//!   same id back later (possibly on the next page), which takes it off the candidates. Unmounting a shared folder or
//!   losing access sends only a single folder `deleted`, so every saved file under it becomes a candidate.
//! - The cursor and the candidates are saved together every page. Even if it stops midway, the next sync continues
//!   reading, so renamed files are not lost.
//! - The check (`changed`) does not save the cursor and reads at most `CHECK_PAGES` pages.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use super::api::{DropboxApi, Failure, Got, failure};
use super::fetch::{Run, depth_one};
use super::reconcile::reconcile_due;
use super::state::{
    State, WINDOW_DAYS, bare_id, in_scope, item_ids, kept_muted, load_state, read_item, under,
    version,
};
use crate::error::{Kind, SourceError};

/// 확인이 읽는 쪽 수예요(모든 커서를 합쳐서). 이만큼 읽고도 남았으면 바뀐 것으로 쳐요.
pub(crate) const CHECK_PAGES: usize = 5;

/// 커서를 끝까지 읽지 못하고 멈춘 이유예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    Done,
    /// 커서가 무효예요: 목록부터 다시 받아요.
    Reset,
    /// 뿌리가 바뀌었어요(422).
    RootChanged,
}

/// 읽을 커서들이에요: Whole은 `[None]`, Split은 커서마다 `Some(key)`(`""`는 뿌리)예요.
pub(crate) fn cursor_keys(s: &State) -> Vec<Option<String>> {
    if s.split() {
        s.cursors.keys().cloned().map(Some).collect()
    } else {
        vec![None]
    }
}

pub(crate) fn cursor_of(s: &State, key: &Option<String>) -> Option<String> {
    match key {
        None => s.cursor.clone(),
        Some(k) => s.cursors.get(k).cloned(),
    }
}

pub(crate) fn set_cursor(s: &mut State, key: &Option<String>, cursor: &str) {
    match key {
        None => s.cursor = Some(cursor.to_string()),
        Some(k) => {
            s.cursors.insert(k.clone(), cursor.to_string());
        }
    }
}

impl Run<'_> {
    /// 커서 한 쪽이에요. 커서가 무효거나(`reset`, 409 `path/…`) 뿌리가 바뀌었으면 `Ok(Err(..))`예요.
    pub(crate) fn continue_page(
        &self,
        cursor: &str,
        root: &str,
    ) -> Result<Result<Value, Ended>, SourceError> {
        match self.api.rpc(
            "files/list_folder/continue",
            &json!({"cursor": cursor}),
            Some(root),
        )? {
            Got::Fail { status, summary } => match failure(status, &summary) {
                Failure::Reset => Ok(Err(Ended::Reset)),
                Failure::PathRoot => Ok(Err(Ended::RootChanged)),
                _ if status == 409 && summary.starts_with("path/") => Ok(Err(Ended::Reset)),
                _ => Err(SourceError::service(
                    Kind::Down,
                    format!("dropbox: changes failed {status}"),
                )),
            },
            got => got.json("changes").map(Ok),
        }
    }

    /// 커서마다 끝까지 읽어 저장본에 맞춰요. 지울 후보는 다 읽은 뒤에 지워요.
    pub(crate) fn changes(&self, s: &mut State) -> Result<Ended, SourceError> {
        let root = s.root.clone().unwrap_or_default();
        // path_lower → id: 지금 저장본에서 한 번 만들고, 아래에서 고칠 때마다 같이 고쳐요
        let mut by_path: HashMap<String, String> = item_ids(self.dir)
            .into_iter()
            .filter_map(|id| {
                read_item(self.dir, &id)
                    .and_then(|v| v["path_lower"].as_str().map(|p| (p.to_string(), id)))
            })
            .collect();
        let mut doomed = s.doomed.clone();
        for key in cursor_keys(s) {
            // 앞 커서에서 그 맨 위 폴더가 지워졌어요
            let Some(mut cur) = cursor_of(s, &key) else {
                continue;
            };
            loop {
                let page = match self.continue_page(&cur, &root)? {
                    Ok(p) => p,
                    Err(end) => return Ok(end),
                };
                for e in page["entries"].as_array().into_iter().flatten() {
                    let p = e["path_lower"].as_str().unwrap_or("");
                    if p.is_empty() {
                        continue;
                    }
                    match e[".tag"].as_str() {
                        Some("deleted") => {
                            s.vaults
                                .retain(|v| !under(v, p) && !under(&format!("{v}/.obsidian"), p));
                            for (path, id) in &by_path {
                                if under(path, p) {
                                    doomed.insert(id.clone());
                                }
                            }
                            let gone: Vec<String> = s
                                .tops
                                .iter()
                                .filter(|(_, t)| under(&t.path_lower, p))
                                .map(|(k, _)| k.clone())
                                .collect();
                            for k in gone {
                                // 끈 폴더는 끈 채로 둬요: 이름 바꿈·권한 바뀜이면 같은 id가 뒤에 다시 와요
                                s.remember_muted_path(&k);
                                s.tops.remove(&k);
                                s.cursors.remove(&k);
                            }
                        }
                        Some("file") => {
                            let Some(id) = e["id"].as_str().and_then(bare_id) else {
                                continue;
                            };
                            doomed.remove(id);
                            // 파일이 폴더 자리를 대신했어요
                            let below = format!("{p}/");
                            for (path, other) in &by_path {
                                if path.starts_with(&below) {
                                    doomed.insert(other.clone());
                                }
                            }
                            self.upsert(s, e)?;
                            by_path.retain(|_, v| v != id);
                            if read_item(self.dir, id).is_some() {
                                by_path.insert(p.to_string(), id.to_string());
                            }
                        }
                        Some("folder") => {
                            self.upsert_folder(s, e);
                            // Split에서 뿌리 커서에 새 맨 위 폴더가 왔어요: 그 폴더를 받고 커서를 더해요
                            let new_top = e["id"].as_str().and_then(bare_id).filter(|id| {
                                key.as_deref() == Some("")
                                    && depth_one(p)
                                    && !s.cursors.contains_key(*id)
                            });
                            if let Some(id) = new_top {
                                let (entries, c) = self.list_from(p, true, &root)?;
                                for x in &entries {
                                    match x[".tag"].as_str() {
                                        Some("folder") => self.upsert_folder(s, x),
                                        Some("file") => {
                                            let Some(xid) = x["id"].as_str().and_then(bare_id)
                                            else {
                                                continue;
                                            };
                                            doomed.remove(xid);
                                            self.upsert(s, x)?;
                                            if let Some(xp) = x["path_lower"].as_str()
                                                && read_item(self.dir, xid).is_some()
                                            {
                                                by_path.retain(|_, v| v != xid);
                                                by_path.insert(xp.to_string(), xid.to_string());
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                s.cursors.insert(id.to_string(), c);
                            }
                        }
                        _ => {}
                    }
                }
                let more = page["has_more"].as_bool() == Some(true);
                match page["cursor"].as_str() {
                    Some(c) => cur = c.to_string(),
                    None if more => {
                        return Err(SourceError::service(Kind::Down, "dropbox: no cursor"));
                    }
                    None => {}
                }
                // 이 커서의 맨 위 폴더가 지워졌으면 커서를 되살리지 않고 그만 읽어요
                let dropped = key.as_ref().is_some_and(|k| !s.cursors.contains_key(k));
                if !dropped {
                    set_cursor(s, &key, &cur);
                }
                s.doomed = doomed.clone();
                // 쪽을 다 처리한 뒤에만(빈 쪽도) 커서와 후보를 같이 저장해요
                self.save(s)?;
                if dropped || !more {
                    break;
                }
            }
        }
        for id in std::mem::take(&mut doomed) {
            self.remove(&id)?;
            s.pending.retain(|p| *p != id);
            s.retries.remove(&id);
        }
        s.doomed.clear();
        s.muted_moved.clear();
        self.save(s)?;
        Ok(Ended::Done)
    }

    /// 확인에서 이 항목이 저장본을 바꾸는지예요.
    fn matters(&self, s: &State, key: &Option<String>, e: &Value, stored: &[String]) -> bool {
        let p = e["path_lower"].as_str().unwrap_or("");
        if p.is_empty() {
            return false;
        }
        match e[".tag"].as_str() {
            Some("deleted") => stored.iter().any(|path| under(path, p)),
            Some("file") => {
                let Some(id) = e["id"].as_str().and_then(bare_id) else {
                    return false;
                };
                let old = read_item(self.dir, id);
                if in_scope(e, self.floor, &s.muted_paths()) {
                    return old.is_none_or(|o| {
                        version(&o) != version(e) || o["path_lower"].as_str() != Some(p)
                    });
                }
                let Some(old) = old else { return false };
                // 끈 폴더 안에서 고친 파일은 저장본을 그대로 둬요(`upsert`와 같아요). 경로가 바뀌었으면 고쳐야 해요
                let old_path = old["path_lower"].as_str().unwrap_or("");
                !(kept_muted(s, self.floor, e, old_path) && old_path == p)
            }
            Some("folder") => {
                key.as_deref() == Some("")
                    && depth_one(p)
                    && e["id"]
                        .as_str()
                        .and_then(bare_id)
                        .is_some_and(|id| !s.cursors.contains_key(id))
            }
            _ => false,
        }
    }
}

/// 30분마다의 확인이에요: 동기화할 것이 있으면 참이에요. 커서는 저장하지 않아요.
pub fn changed(
    api: &dyn DropboxApi,
    raw_dir: &Path,
    now: DateTime<Utc>,
) -> Result<bool, SourceError> {
    let s = load_state(raw_dir);
    if !s.initial_done
        || !s.pending.is_empty()
        || !s.rescan.is_empty()
        || reconcile_due(&s, now)
        || !s.has_cursor()
    {
        return Ok(true);
    }
    let run = Run {
        api,
        dir: raw_dir,
        still: &|| true,
        account: None,
        floor: now - Duration::days(WINDOW_DAYS),
        now,
    };
    let root = s.root.clone().unwrap_or_default();
    let stored: Vec<String> = item_ids(raw_dir)
        .iter()
        .filter_map(|id| {
            read_item(raw_dir, id)?["path_lower"]
                .as_str()
                .map(str::to_string)
        })
        .collect();
    let mut pages = 0;
    for key in cursor_keys(&s) {
        let Some(mut cur) = cursor_of(&s, &key) else {
            continue;
        };
        loop {
            if pages == CHECK_PAGES {
                return Ok(true);
            }
            pages += 1;
            let page = match run.continue_page(&cur, &root)? {
                Ok(p) => p,
                Err(_) => return Ok(true),
            };
            let entries = page["entries"].as_array().into_iter().flatten();
            if entries
                .into_iter()
                .any(|e| run.matters(&s, &key, e, &stored))
            {
                return Ok(true);
            }
            if page["has_more"].as_bool() != Some(true) {
                break;
            }
            cur = page["cursor"].as_str().unwrap_or(&cur).to_string();
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dropbox::fetch::fetch_with;
    use crate::dropbox::state::{item_ids, load_state, read_item, set_muted};
    use crate::gdrive::fetch::{BATCH_FILES, BATCH_TIME, Budget};
    use crate::testing::dropbox::FakeDropbox;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap()
    }

    fn run(f: &FakeDropbox, dir: &Path) -> Result<bool, SourceError> {
        let mut b = Budget::new(BATCH_FILES, BATCH_TIME);
        fetch_with(
            f,
            dir,
            now(),
            Some("dbid:A"),
            &|| true,
            true,
            &mut b,
            &mut |_, _| {},
        )
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/extract/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn downloads(f: &FakeDropbox) -> Vec<String> {
        f.calls()
            .into_iter()
            .filter(|c| c.starts_with("content "))
            .collect()
    }

    fn path_lower(dir: &Path, id: &str) -> String {
        read_item(dir, id).unwrap()["path_lower"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// 저장한 커서부터 온 변경을 읽어요(커서는 그대로예요).
    fn pending_changes(f: &FakeDropbox, dir: &Path) -> Vec<Value> {
        let cursor = load_state(dir).cursor.unwrap();
        f.rpc(
            "files/list_folder/continue",
            &json!({"cursor": cursor}),
            Some("100"),
        )
        .unwrap()
        .json("continue")
        .unwrap()["entries"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn sample() -> FakeDropbox {
        let f = FakeDropbox::at(now());
        f.file("p", "/시험/note.pdf", 3, &fixture("policy.pdf"));
        f.file("w", "/시험/doc.docx", 3, &fixture("doc.docx"));
        f.file(
            "m",
            "/시험/결정-기록.md",
            3,
            "# 결정\n금요일 배포 금지".as_bytes(),
        );
        f
    }

    #[test]
    fn an_edit_is_fetched_again_and_a_rename_only_moves_the_item() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        f.clear_calls();
        f.edit("m", "# 결정\n목요일 배포 금지".as_bytes());
        f.rename("w", "/시험/새 이름.docx");
        run(&f, d.path()).unwrap();
        assert_eq!(
            read_item(d.path(), "m").unwrap()["text"],
            "# 결정\n목요일 배포 금지"
        );
        let w = read_item(d.path(), "w").unwrap();
        assert_eq!(w["path_display"], "/시험/새 이름.docx");
        assert_eq!(downloads(&f), ["content files/download id:m"]);
        assert!(load_state(d.path()).doomed.is_empty());
    }

    #[test]
    fn an_obsidian_folder_marks_its_parent_as_a_vault_until_it_is_deleted() {
        let f = sample();
        f.folder("v", "/일/노트");
        f.folder("o", "/일/노트/.obsidian");
        f.file("n", "/일/노트/결정.md", 3, "# 결정".as_bytes());
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        assert_eq!(
            load_state(d.path()).vaults.into_iter().collect::<Vec<_>>(),
            ["/일/노트"]
        );
        f.delete("/일/노트/.obsidian");
        run(&f, d.path()).unwrap();
        assert!(load_state(d.path()).vaults.is_empty());
    }

    #[test]
    fn deleting_a_file_or_a_folder_removes_what_was_under_it() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        f.delete("/시험/note.pdf");
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["m", "w"]);
        f.delete("/시험");
        run(&f, d.path()).unwrap();
        assert!(item_ids(d.path()).is_empty());
        let s = load_state(d.path());
        assert!(s.tops.is_empty() && s.doomed.is_empty());
    }

    #[test]
    fn deleting_a_folder_leaves_its_prefix_sibling_alone() {
        let f = FakeDropbox::at(now());
        f.file("a1", "/a/x.md", 1, b"a");
        f.file("b1", "/ab/x.md", 1, b"b");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        f.delete("/a");
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["b1"]);
        let tops: Vec<String> = load_state(d.path())
            .tops
            .values()
            .map(|t| t.path_lower.clone())
            .collect();
        assert_eq!(tops, ["/ab"]);
    }

    #[test]
    fn unmounting_a_shared_folder_sends_only_the_folder_and_still_removes_its_files() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        f.join("/B 공유 폴더", "fold1", &[("s1", "x.md"), ("s2", "y.pdf")]);
        run(&f, d.path()).unwrap();
        assert!(read_item(d.path(), "s1").is_some() && read_item(d.path(), "s2").is_some());
        f.unmount("/B 공유 폴더");
        let log = pending_changes(&f, d.path());
        assert_eq!(log.len(), 1);
        assert_eq!(log[0][".tag"], "deleted");
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["m", "p", "w"]);
        let s = load_state(d.path());
        assert!(!s.tops.contains_key("fold1") && s.doomed.is_empty());
    }

    #[test]
    fn rejoining_keeps_the_same_file_ids_without_downloading_again() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        let files = [("s1", "x.md"), ("s2", "y.md")];
        f.join("/B 공유 폴더", "fold1", &files);
        run(&f, d.path()).unwrap();
        f.unmount("/B 공유 폴더");
        f.join("/B 공유 폴더", "fold2", &files);
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert!(read_item(d.path(), "s1").is_some() && read_item(d.path(), "s2").is_some());
        assert!(downloads(&f).is_empty());
        let s = load_state(d.path());
        assert!(s.tops.contains_key("fold2") && !s.tops.contains_key("fold1"));
        assert!(s.doomed.is_empty());
    }

    #[test]
    fn moving_a_folder_keeps_ids_and_moves_paths() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        f.clear_calls();
        f.move_folder("/시험", "/보관/시험");
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["m", "p", "w"]);
        for id in ["m", "p", "w"] {
            assert!(path_lower(d.path(), id).starts_with("/보관/시험/"));
        }
        assert!(downloads(&f).is_empty());
        let tops: Vec<String> = load_state(d.path())
            .tops
            .values()
            .map(|t| t.path_lower.clone())
            .collect();
        assert_eq!(tops, ["/보관"]);
    }

    #[test]
    fn a_rename_split_across_an_interrupted_run_keeps_the_file() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"# m");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        f.set_page(1);
        f.rename("m", "/b/m.md"); // 쪽마다 하나: deleted /a/m.md, folder /b, file /b/m.md
        f.fail_after_pages(1, 503);
        assert!(run(&f, d.path()).is_err());
        assert!(load_state(d.path()).doomed.contains("m"));
        assert!(read_item(d.path(), "m").is_some());
        f.clear_failures();
        run(&f, d.path()).unwrap();
        assert_eq!(read_item(d.path(), "m").unwrap()["path_lower"], "/b/m.md");
        assert!(load_state(d.path()).doomed.is_empty());
    }

    #[test]
    fn a_reset_after_an_interrupted_run_keeps_listed_candidates_and_removes_the_rest() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"# m");
        f.file("k", "/a/k.md", 1, b"# k");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        f.set_page(1);
        f.delete("/a/k.md");
        f.rename("m", "/b/m.md");
        f.fail_after_pages(2, 503); // 두 deleted만 읽고 멈춰요
        assert!(run(&f, d.path()).is_err());
        assert_eq!(load_state(d.path()).doomed.len(), 2);
        f.clear_failures();
        f.set_page(2000);
        f.expire_cursors();
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["m"]);
        assert_eq!(path_lower(d.path(), "m"), "/b/m.md");
        assert!(load_state(d.path()).doomed.is_empty());
    }

    #[test]
    fn a_file_moved_under_a_muted_folder_is_removed_and_edits_inside_it_are_ignored() {
        let f = FakeDropbox::at(now());
        f.folder("f1", "/a");
        f.file("y", "/a/y.md", 1, b"# y");
        f.file("x", "/b/x.md", 1, b"# x");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        set_muted(d.path(), &["f1".to_string()]).unwrap();
        f.clear_calls();
        f.rename("x", "/a/x.md");
        f.edit("y", b"# y2");
        run(&f, d.path()).unwrap();
        assert!(read_item(d.path(), "x").is_none());
        assert_eq!(read_item(d.path(), "y").unwrap()["text"], "# y");
        assert!(downloads(&f).is_empty());
        let s = load_state(d.path());
        assert!(s.pending.is_empty() && s.doomed.is_empty());
    }

    /// 조각이 나오는 저장본 id예요.
    fn chunk_ids(dir: &Path) -> Vec<String> {
        let mut ids: Vec<String> = crate::dropbox::docs::chunks_of(dir, now())
            .unwrap()
            .iter()
            .map(|c| c["path"].as_str().unwrap().to_string())
            .collect();
        ids.dedup();
        ids
    }

    /// `/a`(f1, 끔) 아래 y, `/b` 아래 x를 받고 f1을 끈 뒤 y를 고쳐요(끈 동안은 받지 않아요).
    fn muted_a() -> (FakeDropbox, tempfile::TempDir) {
        let f = FakeDropbox::at(now());
        f.folder("f1", "/a");
        f.file("y", "/a/y.md", 1, b"# y");
        f.file("x", "/b/x.md", 1, b"# x");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        set_muted(d.path(), &["f1".to_string()]).unwrap();
        f.edit("y", b"# y2");
        (f, d)
    }

    fn assert_y_kept_at(dir: &Path, path: &str) {
        let y = read_item(dir, "y").unwrap();
        assert_eq!(y["text"], "# y");
        assert_eq!(y["path_lower"], path);
        assert_eq!(y["path_display"], path);
        assert_eq!(y["name"], "y.md");
        let s = load_state(dir);
        assert!(s.pending.is_empty() && s.doomed.is_empty());
        assert_eq!(s.muted, ["f1"]);
        assert_eq!(s.tops["f1"].path_lower, path.rsplit_once('/').unwrap().0);
        assert!(s.muted_moved.is_empty());
        assert_eq!(chunk_ids(dir), ["dropbox/x"]);
    }

    #[test]
    fn a_renamed_muted_top_folder_is_not_fetched_and_keeps_its_files_at_the_new_path() {
        let (f, d) = muted_a();
        f.move_folder("/a", "/a2");
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert!(downloads(&f).is_empty(), "{:?}", downloads(&f));
        assert_y_kept_at(d.path(), "/a2/y.md");
        // 다시 켜면 그 폴더를 다시 훑어 고친 파일을 받아요
        set_muted(d.path(), &[]).unwrap();
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert_eq!(downloads(&f), ["content files/download id:y"]);
        assert_eq!(read_item(d.path(), "y").unwrap()["text"], "# y2");
        assert_eq!(chunk_ids(d.path()), ["dropbox/x", "dropbox/y"]);
    }

    #[test]
    fn a_muted_top_rename_split_across_an_interrupted_run_is_still_not_fetched() {
        let (f, d) = muted_a();
        f.set_page(1);
        f.move_folder("/a", "/a2"); // 쪽마다 하나: deleted y, deleted /a, folder /a2, file y
        f.fail_after_pages(2, 503);
        assert!(run(&f, d.path()).is_err());
        f.clear_failures();
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert!(downloads(&f).is_empty(), "{:?}", downloads(&f));
        assert_y_kept_at(d.path(), "/a2/y.md");
    }

    #[test]
    fn a_permission_change_on_a_muted_shared_folder_downloads_nothing() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        f.join("/B 공유 폴더", "fold1", &[("s1", "x.md"), ("s2", "y.md")]);
        run(&f, d.path()).unwrap();
        let before = (read_item(d.path(), "s1"), read_item(d.path(), "s2"));
        set_muted(d.path(), &["fold1".to_string()]).unwrap();
        f.edit("s1", "# 바뀐 글".as_bytes());
        f.reshare("/B 공유 폴더");
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert!(downloads(&f).is_empty(), "{:?}", downloads(&f));
        assert_eq!(
            (read_item(d.path(), "s1"), read_item(d.path(), "s2")),
            before
        );
        let s = load_state(d.path());
        assert!(s.pending.is_empty() && s.doomed.is_empty());
        assert_eq!(s.muted, ["fold1"]);
        assert!(s.tops.contains_key("fold1"));
        assert_eq!(chunk_ids(d.path()), ["dropbox/m", "dropbox/p", "dropbox/w"]);
    }

    #[test]
    fn a_file_older_than_two_years_after_an_edit_is_removed() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        f.edit_at("m", 731);
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["p", "w"]);
    }

    #[test]
    fn the_cursor_is_saved_even_when_nothing_changed() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        let first = load_state(d.path()).cursor.unwrap();
        // 받지 않는 파일만 바뀌었어요: 저장본은 그대로이고 커서만 앞으로 가요
        f.file("i", "/시험/사진.png", 1, b"png");
        run(&f, d.path()).unwrap();
        let second = load_state(d.path()).cursor.unwrap();
        assert_ne!(second, first);
        assert_eq!(item_ids(d.path()), ["m", "p", "w"]);
        let latest = f
            .rpc(
                "files/list_folder",
                &json!({"path": "", "recursive": true}),
                Some("100"),
            )
            .unwrap()
            .json("list")
            .unwrap()["cursor"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(second, latest);
        run(&f, d.path()).unwrap();
        assert_eq!(load_state(d.path()).cursor.unwrap(), latest);
    }

    #[test]
    fn the_check_sees_only_changes_that_matter_and_stops_after_five_pages() {
        let f = sample();
        let d = tempfile::tempdir().unwrap();
        let check = || changed(&f, d.path(), now()).unwrap();
        run(&f, d.path()).unwrap();
        let cursor = || load_state(d.path()).cursor;
        let before = cursor();
        assert!(!check());
        f.file("i", "/시험/사진.png", 1, b"png");
        assert!(!check());
        assert_eq!(cursor(), before, "the check never writes the cursor");
        f.file("n", "/시험/새 메모.md", 1, b"# n");
        assert!(check());
        assert_eq!(cursor(), before);
        run(&f, d.path()).unwrap();
        assert!(!check());
        f.delete("/시험/note.pdf");
        assert!(check());
        run(&f, d.path()).unwrap();
        assert!(!check());
        f.expire_cursors();
        assert!(check());
        run(&f, d.path()).unwrap();
        assert!(!check());
        f.set_page(1);
        for k in 0..6 {
            f.file(&format!("g{k}"), &format!("/시험/그림{k}.png"), 1, b"png");
        }
        f.clear_calls();
        let before = cursor();
        assert!(check());
        assert_eq!(cursor(), before);
        let pages = f
            .calls()
            .iter()
            .filter(|c| *c == "rpc files/list_folder/continue")
            .count();
        assert_eq!(pages, CHECK_PAGES);
    }
}
