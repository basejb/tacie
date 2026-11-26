//! A large sync, like the first one, is added to search in parts as it is fetched (refresh.rs `embed_in_parts`).

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::sync::{Arc, Mutex};

use common::sandbox::Sandbox;
use tacie_engine::refresh::{Target, seed_meta};

/// 노션 페이지 여섯 개를 받는 씨앗 색인이에요: 두 개씩 나눠 검색에 넣어요.
fn notion_pages(s: &Sandbox) {
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    let mut pages = s.notion_api.pages.lock().unwrap();
    for i in 0..6 {
        pages.push((
            format!("p-{i}"),
            format!("페이지 {i}"),
            format!("본문 {i}이에요."),
        ));
    }
    drop(pages);
    s.notion.set(Some("token"), Some("팀"));
}

#[test]
fn a_big_sync_is_searchable_part_by_part() {
    let s = Sandbox::with_partial(4, 2);
    notion_pages(&s);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    let log = s.r.status_log();
    // 나눠 넣은 뒤에는 상태가 그렇게 말해요
    assert!(log.iter().any(|v| v["partial"] == true), "{log:?}");
    // 끝나면 전부예요
    assert_eq!(s.engine.index.load().raw.len(), 6);
    assert!(s.meta()["built_at"].is_string());
}

#[test]
fn a_small_sync_swaps_once() {
    let s = Sandbox::with_partial(100, 2);
    notion_pages(&s);
    s.sync(Target::default());
    assert!(s.r.status_log().iter().all(|v| v.get("partial").is_none()));
    assert_eq!(s.engine.index.load().raw.len(), 6);
}

#[test]
fn a_part_keeps_what_the_live_index_had_for_documents_not_ready_yet() {
    let s = Sandbox::with_partial(4, 2);
    notion_pages(&s);
    s.sync(Target::default());
    // 모든 페이지가 바뀌었어요: 앞 묶음이 들어가는 동안 나머지 페이지는 옛 글로 찾아요
    for p in s.notion_api.pages.lock().unwrap().iter_mut() {
        p.2 = format!("{} 고쳤어요.", p.2);
    }
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let (dir, seen) = (s.index_dir(), sizes.clone());
    s.on_open(Some(Box::new(move || {
        let chunks: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.join("chunks.json")).unwrap()).unwrap();
        seen.lock().unwrap().push(chunks.len());
    })));
    // 저장본의 수정 시각이 같아서 다시 받지 않아요: 지우고 모두 다시 받아요
    tacie_sources::notion::forget(&s.r.notion_dir()).unwrap();
    s.sync(Target::default());
    let sizes = sizes.lock().unwrap().clone();
    assert!(sizes.len() >= 3, "parts and the end: {sizes:?}");
    // 어느 때든 여섯 페이지가 다 찾아져요(새 글이거나 옛 글이거나)
    assert!(sizes.iter().all(|&n| n == 6), "{sizes:?}");
    assert!(s.texts().contains("고쳤어요"));
}

#[test]
fn switching_repos_never_shows_the_old_repo_under_the_new_name() {
    let s = Sandbox::with_partial(1, 1);
    assert!(s.texts().contains("o/old"));
    let leaked = Arc::new(Mutex::new(Vec::new()));
    let (dir, seen) = (s.index_dir(), leaked.clone());
    s.on_open(Some(Box::new(move || {
        let text = std::fs::read_to_string(dir.join("chunks.json")).unwrap();
        seen.lock().unwrap().push(text.contains("o/old"));
    })));
    let st = s.run(Target {
        repo: Some("o/new".into()),
        branch: Some("main".into()),
        fresh: false,
    });
    assert_eq!(st["state"], "done", "{st}");
    let leaked = leaked.lock().unwrap().clone();
    assert!(leaked.len() >= 2, "parts and the end: {leaked:?}");
    // 중간 색인에도 옛 저장소의 조각은 없어요(새 저장소 이름으로 링크가 걸려요)
    assert!(leaked.iter().all(|&old| !old), "{leaked:?}");
}

#[test]
fn disconnecting_github_stops_the_sync_and_puts_the_old_index_back() {
    let s = Sandbox::with_partial(1, 1);
    let before = s.texts();
    assert!(before.contains("o/old"));
    // the first part goes into search, then the user disconnects GitHub (설정 > 연결 > 연결 해제)
    let github = s.github.clone();
    let opened = Arc::new(Mutex::new(0));
    let count = opened.clone();
    s.on_open(Some(Box::new(move || {
        let mut n = count.lock().unwrap();
        *n += 1;
        if *n == 1 {
            github.set_credentials("none", None);
        }
    })));
    let st = s.run(Target {
        repo: Some("o/new".into()),
        branch: Some("main".into()),
        fresh: false,
    });
    // stopped, not failed: nothing went wrong, and the app shows no problem
    assert_eq!(st["state"], "idle", "{st}");
    assert_eq!(st["cancelled"], true, "{st}");
    // the search data is the one from before the sync, not the part of the new repo
    assert_eq!(s.texts(), before);
    assert_eq!(s.meta()["repo"], "o/old");
    assert!(!s.index_dir().with_file_name("index.keep").exists());
}

#[test]
fn a_sync_that_keeps_its_connection_leaves_no_kept_index() {
    let s = Sandbox::with_partial(1, 1);
    let st = s.run(Target {
        repo: Some("o/new".into()),
        branch: Some("main".into()),
        fresh: false,
    });
    assert_eq!(st["state"], "done", "{st}");
    assert!(!s.index_dir().with_file_name("index.keep").exists());
}
