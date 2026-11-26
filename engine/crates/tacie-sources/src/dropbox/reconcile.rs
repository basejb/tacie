//! Fetches the Dropbox listing again from the start and matches the saved copy to it (docs/33 6절).
//!
//! - Reasons to refetch (`Why`): initial fetch, invalid cursor (reset), root change (joined a team), weekly reconcile.
//! - The saved copy is changed only after the new listing has been fetched to the end. If even one page fails, nothing
//!   is deleted and the old cursor stays.
//! - Saved files missing from the listing are deleted by the `settle` rule: if few are missing, at once; if many (5 or
//!   more and over 20%) are missing, on the second reconcile. If the listing has no files at all, nothing is deleted.
//! - If one recursive listing fails `SPLIT_AFTER` times, each top folder gets its own cursor (Split; the root cursor
//!   is `""`).
//! - A re-enabled folder (`rescan`) is fetched once on its own and missing files are added (cursor and deletes stay
//!   as they are).

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Duration, Utc};

use super::fetch::{Listing, Run, depth_one};
use super::state::{
    ITEMS, RECONCILE_DAYS, State, bare_id, item_ids, load_state, read_item, save_state, state_lock,
};
use crate::error::SourceError;
use crate::gdrive::fetch::{iso, parse};

/// 많이 빠진 저장본을 지우기까지의 재조정 수예요.
pub(crate) const STRIKES: u8 = 2;
/// 재귀 목록이 이만큼 실패하면 폴더마다 받아요.
pub(crate) const SPLIT_AFTER: u32 = 2;

/// 목록을 다시 받는 이유예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    Initial,
    Reset,
    Root,
    Weekly,
}

/// 빠진 것을 지우는 규칙이에요. `missed`는 이번 목록에서 빠진 저장본(모두 `held`개 중)이에요. 조금 빠졌거나 이미
/// `STRIKES - 1`번 빠진 것은 돌려주고(지워요) `gone`에서 빼요. 나머지는 빠짐 횟수를 올려요. `missed`에 없는 것은 다시
/// 나왔으니 `gone`에서 지워요.
pub(crate) fn settle(
    gone: &mut BTreeMap<String, u8>,
    missed: &[String],
    held: usize,
) -> Vec<String> {
    let small = missed.len() < 5 || missed.len() * 5 <= held;
    let now: HashSet<&str> = missed.iter().map(String::as_str).collect();
    gone.retain(|k, _| now.contains(k.as_str()));
    let mut out = Vec::new();
    for id in missed {
        let strikes = gone.get(id).copied().unwrap_or(0);
        if small || strikes.saturating_add(1) >= STRIKES {
            gone.remove(id);
            out.push(id.clone());
        } else {
            gone.insert(id.clone(), strikes + 1);
        }
    }
    out
}

/// 주간 재조정을 할 때가 됐는지예요: 처음 받기를 끝냈고, 지난 재조정에서 `RECONCILE_DAYS`가 지났거나 기록이 없어요.
pub(crate) fn reconcile_due(s: &State, now: DateTime<Utc>) -> bool {
    s.initial_done
        && s.reconciled_at
            .as_deref()
            .and_then(parse)
            .is_none_or(|t| now - t >= Duration::days(RECONCILE_DAYS))
}

impl Run<'_> {
    /// 저장본 하나를 지워요: 받을 목록과 다시 받기 횟수에서도 빼요.
    fn drop_item(&self, s: &mut State, id: &str) -> Result<(), SourceError> {
        self.remove(id)?;
        s.pending.retain(|p| p != id);
        s.retries.remove(id);
        Ok(())
    }

    /// 폴더마다 받아요: 뿌리(`""`)는 재귀 없이, 그 목록의 맨 위 폴더는 하나씩 재귀로요(키 = 폴더 id).
    pub(crate) fn list_split(&self, root: &str) -> Result<Listing, SourceError> {
        let (mut entries, cursor) = self.list_from("", false, root)?;
        let mut cursors = BTreeMap::from([(String::new(), cursor)]);
        let tops: Vec<(String, String)> = entries
            .iter()
            .filter(|e| e[".tag"] == "folder")
            .filter_map(|e| {
                let id = e["id"].as_str().and_then(bare_id)?;
                let path = e["path_lower"].as_str().filter(|p| depth_one(p))?;
                Some((id.to_string(), path.to_string()))
            })
            .collect();
        for (id, path) in tops {
            let (more, cursor) = self.list_from(&path, true, root)?;
            entries.extend(more);
            cursors.insert(id, cursor);
        }
        Ok(Listing {
            entries,
            cursor: None,
            cursors,
        })
    }

    /// 목록을 처음부터 끝까지 다시 받아 저장본에 맞춰요. 목록을 끝까지 받지 못하면 아무것도 바꾸지 않아요.
    pub(crate) fn relist(&self, s: &mut State, why: Why) -> Result<(), SourceError> {
        let listing = self.list_all(s)?;
        {
            // 목록까지 받았으면 저장본이 있는 것으로 쳐요
            let _held = state_lock();
            self.guard()?;
            std::fs::create_dir_all(self.dir.join(ITEMS))?;
        }
        // 끈 폴더의 지금 경로를 기억해요: 이름이 바뀌었으면 그 아래 저장본은 경로만 고쳐요
        for id in s.muted.clone() {
            s.remember_muted_path(&id);
        }
        // 폴더를 먼저, 그다음 파일을 넣어요. vault는 목록에서 다시 찾아요
        s.vaults.clear();
        let mut folders = HashSet::new();
        for e in listing.entries.iter().filter(|e| e[".tag"] == "folder") {
            self.upsert_folder(s, e);
            folders.extend(e["id"].as_str().and_then(bare_id).map(str::to_string));
        }
        s.tops.retain(|id, _| folders.contains(id));
        let mut listed = HashSet::new();
        for e in listing.entries.iter().filter(|e| e[".tag"] == "file") {
            self.upsert(s, e)?;
            listed.extend(e["id"].as_str().and_then(bare_id).map(str::to_string));
        }
        // 커서를 읽다 멈춘 지울 후보 중 목록에 없는 것은 바로 지워요
        for id in std::mem::take(&mut s.doomed) {
            if !listed.contains(&id) {
                self.drop_item(s, &id)?;
            }
        }
        if !listed.is_empty() {
            let held = item_ids(self.dir);
            let missed: Vec<String> = held
                .iter()
                .filter(|id| !listed.contains(*id))
                .cloned()
                .collect();
            for id in settle(&mut s.gone, &missed, held.len()) {
                self.drop_item(s, &id)?;
            }
        }
        if why == Why::Weekly {
            // 목록에 오지 않아 `upsert`가 보지 못한 2년 지난 저장본이에요
            for id in item_ids(self.dir) {
                let old = read_item(self.dir, &id).is_some_and(|v| {
                    v["server_modified"]
                        .as_str()
                        .and_then(parse)
                        .is_some_and(|t| t <= self.floor)
                });
                if old {
                    self.drop_item(s, &id)?;
                }
            }
            s.reconciled_at = Some(iso(self.now));
        }
        s.cursor = listing.cursor;
        s.cursors = listing.cursors;
        s.doomed.clear();
        s.muted_moved.clear();
        self.save(s)
    }

    /// 다시 켠 폴더 하나를 받아 빠진 파일을 더해요. 지우지 않고 커서도 그대로예요. 없는 폴더는 그냥 빼요.
    pub(crate) fn rescan(&self, s: &mut State, folder: &str) -> Result<(), SourceError> {
        if let Some(top) = s.tops.get(folder).cloned() {
            let root = s.root.clone().unwrap_or_default();
            let (entries, _) = self.list_from(&top.path_lower, true, &root)?;
            for e in &entries {
                match e[".tag"].as_str() {
                    Some("folder") => self.upsert_folder(s, e),
                    Some("file") => self.upsert(s, e)?,
                    _ => {}
                }
            }
        }
        s.rescan.remove(folder);
        self.save(s)?;
        // `save`는 디스크의 `rescan`을 그대로 둬서 여기서 따로 빼요
        let _held = state_lock();
        self.guard()?;
        let mut disk = load_state(self.dir);
        if disk.rescan.remove(folder) {
            save_state(self.dir, &disk)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};

    use serde_json::{Value, json};

    use crate::dropbox::api::{DropboxApi, Got};
    use crate::dropbox::fetch::fetch_with;
    use crate::dropbox::state::{load_state, save_state, set_muted};
    use crate::error::Kind;
    use crate::gdrive::fetch::{BATCH_FILES, BATCH_TIME, Budget};
    use crate::testing::dropbox::FakeDropbox;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap()
    }

    fn run_at(api: &dyn DropboxApi, dir: &Path, at: DateTime<Utc>) -> Result<bool, SourceError> {
        let mut b = Budget::new(BATCH_FILES, BATCH_TIME);
        fetch_with(
            api,
            dir,
            at,
            Some("dbid:A"),
            &|| true,
            true,
            &mut b,
            &mut |_, _| {},
        )
    }

    fn run(api: &dyn DropboxApi, dir: &Path) -> Result<bool, SourceError> {
        run_at(api, dir, now())
    }

    fn days(n: i64) -> DateTime<Utc> {
        now() + Duration::days(n)
    }

    fn downloads(f: &FakeDropbox) -> Vec<String> {
        f.calls()
            .into_iter()
            .filter(|c| c.starts_with("content "))
            .collect()
    }

    /// 지금 재귀 목록의 마지막 커서예요.
    fn latest(f: &FakeDropbox) -> String {
        let mut v = f
            .rpc(
                "files/list_folder",
                &json!({"path": "", "recursive": true}),
                Some("100"),
            )
            .unwrap()
            .json("list")
            .unwrap();
        while v["has_more"] == true {
            v = f
                .rpc(
                    "files/list_folder/continue",
                    &json!({"cursor": v["cursor"]}),
                    Some("100"),
                )
                .unwrap()
                .json("continue")
                .unwrap();
        }
        v["cursor"].as_str().unwrap().to_string()
    }

    /// 파일 `n`개(`f0`…), 모두 `/a` 아래예요.
    fn many(n: usize) -> FakeDropbox {
        let f = FakeDropbox::at(now());
        for i in 0..n {
            f.file(
                &format!("f{i}"),
                &format!("/a/{i}.md"),
                1,
                format!("# {i}").as_bytes(),
            );
        }
        f
    }

    /// `path`로 가는 첫 호출 하나만 `reply`로 바꿔요. 나머지는 가짜 Dropbox 그대로예요.
    struct Hiccup<'a> {
        inner: &'a FakeDropbox,
        path: &'static str,
        once: AtomicBool,
        reply: Got,
    }

    impl DropboxApi for Hiccup<'_> {
        fn rpc(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
            if path == self.path && self.once.swap(false, Ordering::SeqCst) {
                return Ok(self.reply.clone());
            }
            self.inner.rpc(path, arg, root)
        }
        fn content(&self, path: &str, arg: &Value, root: Option<&str>) -> Result<Got, SourceError> {
            self.inner.content(path, arg, root)
        }
    }

    #[test]
    fn settle_deletes_small_losses_at_once_and_big_ones_on_the_second_miss() {
        let mut gone = BTreeMap::new();
        assert_eq!(settle(&mut gone, &["a".into()], 100), ["a"]); // < 5
        let six: Vec<String> = (0..6).map(|i| i.to_string()).collect();
        assert_eq!(settle(&mut gone, &six, 30), six); // 6*5 <= 30 (20%)
        assert!(settle(&mut gone, &six, 29).is_empty()); // 20% 넘음: 첫 번째
        assert_eq!(gone.len(), 6);
        assert_eq!(settle(&mut gone, &six, 29), six); // 두 번째
        assert!(gone.is_empty());
        // 다시 나온 것은 빠짐 횟수를 지워요
        assert!(settle(&mut gone, &six, 29).is_empty());
        assert!(settle(&mut gone, &[], 29).is_empty());
        assert!(gone.is_empty());
    }

    #[test]
    fn reconcile_is_due_a_week_after_the_last_one_once_the_first_fetch_is_done() {
        let mut s = State::default();
        assert!(!reconcile_due(&s, now()));
        s.initial_done = true;
        assert!(reconcile_due(&s, now()));
        s.reconciled_at = Some(iso(now()));
        assert!(!reconcile_due(&s, days(6)));
        assert!(reconcile_due(&s, days(7)));
    }

    #[test]
    fn the_weekly_reconcile_removes_vanished_files_and_old_ones_and_takes_the_new_cursor() {
        let f = many(9);
        f.file("old", "/a/old.md", 725, b"# old");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()).len(), 10);
        f.vanish("f3");
        f.clear_calls();
        run_at(&f, d.path(), days(8)).unwrap();
        let ids = item_ids(d.path());
        assert_eq!(ids.len(), 8);
        assert!(!ids.contains(&"f3".to_string()) && !ids.contains(&"old".to_string()));
        assert!(f.calls().iter().any(|c| c == "rpc files/list_folder  true"));
        let s = load_state(d.path());
        assert_eq!(s.reconciled_at, Some(iso(days(8))));
        assert_eq!(s.cursor, Some(latest(&f)));
        assert!(s.gone.is_empty());
        // 일주일이 안 지났으면 다시 받지 않아요
        f.clear_calls();
        run_at(&f, d.path(), days(10)).unwrap();
        assert!(
            !f.calls()
                .iter()
                .any(|c| c.starts_with("rpc files/list_folder "))
        );
    }

    #[test]
    fn a_big_loss_waits_for_a_second_reconcile_and_an_empty_listing_removes_nothing() {
        let f = many(20);
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        for i in 0..6 {
            f.vanish(&format!("f{i}")); // 6개, 30%
        }
        run_at(&f, d.path(), days(8)).unwrap();
        assert_eq!(item_ids(d.path()).len(), 20);
        assert_eq!(load_state(d.path()).gone.len(), 6);
        run_at(&f, d.path(), days(15)).unwrap();
        assert_eq!(item_ids(d.path()).len(), 14);
        assert!(load_state(d.path()).gone.is_empty());
        for i in 6..20 {
            f.vanish(&format!("f{i}"));
        }
        run_at(&f, d.path(), days(22)).unwrap();
        assert_eq!(item_ids(d.path()).len(), 14);
    }

    #[test]
    fn a_failed_page_changes_nothing() {
        let f = many(10);
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        for i in 0..2 {
            f.vanish(&format!("f{i}"));
        }
        let before = load_state(d.path());
        f.set_page(1);
        // 커서 확인 한 쪽 + 재조정 목록 두 쪽 뒤에 멈춰요
        f.fail_after_pages(3, 503);
        assert!(run_at(&f, d.path(), days(8)).is_err());
        assert_eq!(item_ids(d.path()).len(), 10);
        let s = load_state(d.path());
        assert_eq!(s.cursor, before.cursor);
        assert_eq!(s.gone, before.gone);
        assert_eq!(s.reconciled_at, before.reconciled_at);
    }

    #[test]
    fn a_reset_cursor_keeps_the_saved_files_until_the_new_listing_is_complete() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"# m");
        f.file("k", "/a/k.md", 1, b"# k");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        let ids = item_ids(d.path());
        f.rename("m", "/b/m.md");
        f.expire_cursors();
        f.fail_after_pages(1, 503); // reset 한 쪽 뒤 새 목록이 실패해요
        assert!(run(&f, d.path()).is_err());
        assert_eq!(item_ids(d.path()), ids);
        assert_eq!(read_item(d.path(), "m").unwrap()["path_lower"], "/a/m.md");
        f.clear_failures();
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ids);
        assert_eq!(read_item(d.path(), "m").unwrap()["path_lower"], "/b/m.md");
        assert!(downloads(&f).is_empty());
        assert_eq!(load_state(d.path()).cursor, Some(latest(&f)));
    }

    #[test]
    fn a_new_root_relists_with_the_new_path_root() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"# m");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        f.set_root("200"); // 팀에 들어갔어요
        f.clear_calls();
        run(&f, d.path()).unwrap();
        let s = load_state(d.path());
        assert_eq!(s.root.as_deref(), Some("200"));
        assert!(f.calls().iter().any(|c| c == "rpc files/list_folder  true"));
        assert!(
            !f.calls()
                .iter()
                .any(|c| c == "rpc files/list_folder/continue")
        );
        assert_eq!(item_ids(d.path()), ["m"]);
        // 계정을 읽은 뒤 커서를 읽는 사이에 뿌리가 바뀌었어요(422)
        let api = Hiccup {
            inner: &f,
            path: "files/list_folder/continue",
            once: AtomicBool::new(true),
            reply: Got::Fail {
                status: 422,
                summary: String::new(),
            },
        };
        f.clear_calls();
        run(&api, d.path()).unwrap();
        let calls = f.calls();
        assert_eq!(
            calls
                .iter()
                .filter(|c| *c == "rpc users/get_current_account")
                .count(),
            2
        );
        assert!(calls.iter().any(|c| c == "rpc files/list_folder  true"));
        assert_eq!(item_ids(d.path()), ["m"]);
    }

    #[test]
    fn a_failed_listing_after_a_root_change_keeps_the_old_root_and_cursor() {
        let f = FakeDropbox::at(now());
        f.file("m", "/a/m.md", 1, b"# m");
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        let before = load_state(d.path());
        f.set_root("200");
        f.fail_recursive_root(1);
        assert_eq!(run(&f, d.path()).unwrap_err().kind(), Kind::Down);
        let s = load_state(d.path());
        assert_eq!(s.root.as_deref(), Some("100"));
        assert_eq!(s.cursor, before.cursor);
        assert_eq!(s.whole_failures, 1);
        run(&f, d.path()).unwrap();
        let s = load_state(d.path());
        assert_eq!(s.root.as_deref(), Some("200"));
        assert_eq!(s.whole_failures, 0);
        assert_eq!(item_ids(d.path()), ["m"]);
    }

    #[test]
    fn a_root_change_during_the_listing_is_not_counted_as_a_recursive_failure() {
        let f = many(1);
        let d = tempfile::tempdir().unwrap();
        f.fail_recursive_root(1);
        assert!(run(&f, d.path()).is_err());
        assert_eq!(load_state(d.path()).whole_failures, 1);
        let api = Hiccup {
            inner: &f,
            path: "files/list_folder",
            once: AtomicBool::new(true),
            reply: Got::Fail {
                status: 422,
                summary: String::new(),
            },
        };
        assert_eq!(run(&api, d.path()).unwrap_err().kind(), Kind::Down);
        let s = load_state(d.path());
        assert_eq!(s.whole_failures, 1);
        assert!(!s.split() && s.cursor.is_none());
        run(&f, d.path()).unwrap();
        let s = load_state(d.path());
        assert_eq!(s.whole_failures, 0);
        assert!(!s.split() && s.cursor.is_some());
    }

    #[test]
    fn repeated_recursive_failures_switch_to_one_cursor_per_top_folder() {
        let f = FakeDropbox::at(now());
        f.folder("fa", "/a");
        f.folder("fb", "/b");
        f.file("x", "/a/x.md", 1, b"# x");
        f.file("y", "/b/c/y.md", 1, b"# y");
        f.file("r", "/r.md", 1, b"# r");
        let d = tempfile::tempdir().unwrap();
        f.fail_recursive_root(10);
        let e = run(&f, d.path()).unwrap_err();
        assert_eq!(e.kind(), Kind::Down);
        assert_eq!(load_state(d.path()).whole_failures, 1);
        run(&f, d.path()).unwrap();
        let s = load_state(d.path());
        assert_eq!(s.whole_failures, 2);
        assert!(s.cursor.is_none());
        assert_eq!(
            s.cursors.keys().cloned().collect::<Vec<_>>(),
            ["", "fa", "fb"]
        );
        assert_eq!(item_ids(d.path()), ["r", "x", "y"]);
        assert!(s.initial_done);
        // 새 맨 위 폴더는 커서를 더하고 그 안의 파일을 받아요
        f.folder("nf", "/새 폴더");
        f.file("n", "/새 폴더/n.md", 1, b"# n");
        run(&f, d.path()).unwrap();
        let s = load_state(d.path());
        assert!(s.cursors.contains_key("nf"));
        assert!(read_item(d.path(), "n").is_some());
        // 폴더 안의 수정은 그 폴더의 커서가 받아요
        f.edit("y", b"# y2");
        run(&f, d.path()).unwrap();
        assert_eq!(read_item(d.path(), "y").unwrap()["text"], "# y2");
        // 지우면 커서와 파일이 같이 빠져요
        f.delete("/새 폴더");
        run(&f, d.path()).unwrap();
        let s = load_state(d.path());
        assert!(!s.cursors.contains_key("nf") && !s.tops.contains_key("nf"));
        assert!(read_item(d.path(), "n").is_none());
        assert!(s.doomed.is_empty());
    }

    #[test]
    fn a_top_folder_whose_own_cursor_reports_it_deleted_loses_its_cursor() {
        let f = FakeDropbox::at(now());
        f.folder("fa", "/a");
        f.folder("fb", "/b");
        f.file("x", "/a/x.md", 1, b"# x");
        f.file("y", "/b/y.md", 1, b"# y");
        let d = tempfile::tempdir().unwrap();
        let s = State {
            whole_failures: SPLIT_AFTER,
            ..State::default()
        };
        save_state(d.path(), &s).unwrap();
        run(&f, d.path()).unwrap();
        // 뿌리 커서가 없어서 맨 위 폴더 커서만 지움을 봐요
        let mut s = load_state(d.path());
        s.cursors.remove("");
        save_state(d.path(), &s).unwrap();
        f.delete("/a");
        run(&f, d.path()).unwrap();
        let s = load_state(d.path());
        assert_eq!(s.cursors.keys().cloned().collect::<Vec<_>>(), ["fb"]);
        assert_eq!(item_ids(d.path()), ["y"]);
    }

    #[test]
    fn a_page_with_more_but_no_cursor_is_down() {
        let f = many(1);
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        let api = Hiccup {
            inner: &f,
            path: "files/list_folder/continue",
            once: AtomicBool::new(true),
            reply: Got::Body(br#"{"entries": [], "has_more": true}"#.to_vec()),
        };
        let e = run(&api, d.path()).unwrap_err();
        assert_eq!(e.kind(), Kind::Down);
    }

    #[test]
    fn unmuting_a_folder_scans_it_once_and_fetches_only_what_is_missing() {
        let f = FakeDropbox::at(now());
        f.folder("f1", "/a");
        f.file("x", "/a/x.md", 1, b"# x");
        f.file("y", "/b/y.md", 1, b"# y");
        let d = tempfile::tempdir().unwrap();
        set_muted(d.path(), &["f1".to_string()]).unwrap();
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["y"]);
        let cursor = load_state(d.path()).cursor;
        set_muted(d.path(), &[]).unwrap();
        assert!(load_state(d.path()).rescan.contains("f1"));
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert_eq!(item_ids(d.path()), ["x", "y"]);
        let s = load_state(d.path());
        assert!(s.rescan.is_empty());
        assert_eq!(s.cursor, cursor);
        let lists: Vec<String> = f
            .calls()
            .into_iter()
            .filter(|c| c.starts_with("rpc files/list_folder "))
            .collect();
        assert_eq!(lists, ["rpc files/list_folder /a true"]);
        assert_eq!(downloads(&f), ["content files/download id:x"]);
    }

    #[test]
    fn rescanning_a_folder_that_is_gone_just_drops_it() {
        let f = many(1);
        let d = tempfile::tempdir().unwrap();
        run(&f, d.path()).unwrap();
        set_muted(d.path(), &["zz".to_string()]).unwrap();
        set_muted(d.path(), &[]).unwrap();
        f.clear_calls();
        run(&f, d.path()).unwrap();
        assert!(load_state(d.path()).rescan.is_empty());
        assert!(
            !f.calls()
                .iter()
                .any(|c| c.starts_with("rpc files/list_folder "))
        );
    }
}
