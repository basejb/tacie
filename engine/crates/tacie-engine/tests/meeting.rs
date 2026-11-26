//! The past meetings source (docs/30): meetings alone, with a repo and Notion, deleting meetings, and Notion being
//! disconnected or failing.

mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use axum::http::StatusCode;
use common::sandbox::{Sandbox, finish};
use serde_json::{Value, json};
use tacie_engine::refresh::{Target, seed_meta};
use tacie_sources::meeting::{self, Entry, Meeting};
use tacie_sources::notion;

fn meeting_of(id: &str, decision: &str) -> Meeting {
    Meeting {
        id: id.into(),
        title: format!("회의 {id}"),
        start: "2026-11-03T14:02:10+09:00".into(),
        end: "2026-11-03T14:41:55+09:00".into(),
        summary: vec!["결제 화면을 정리했어요".into()],
        topics: vec![],
        ledger: vec![Entry {
            kind: "decision".into(),
            text: decision.into(),
            owner: None,
            due: None,
            done: false,
            replaces: None,
        }],
    }
}

/// 저장소도 다른 연결도 없는 씨앗 색인이에요.
fn alone() -> Sandbox {
    let s = Sandbox::new();
    s.set_meta(seed_meta());
    s.github.set_credentials("none", None);
    s
}

fn paths(s: &Sandbox, src: &str) -> Vec<String> {
    s.engine
        .index
        .load()
        .raw
        .iter()
        .filter(|c| c["src"] == src)
        .map(|c| c["path"].as_str().unwrap().to_string())
        .collect()
}

fn sources(s: &Sandbox) -> Value {
    s.meta()["sources"].clone()
}

#[test]
fn meetings_only_builds_index() {
    let s = alone();
    meeting::save(
        &s.r.meeting_dir(),
        &[
            meeting_of("a", "카드 결제만 먼저 내요"),
            meeting_of("b", "환불은 다음에 해요"),
        ],
    )
    .unwrap();
    s.sync(Target::default());
    assert!(s.texts().contains("결정: 카드 결제만 먼저 내요"));
    assert_eq!(paths(&s, "meeting"), ["meeting/a", "meeting/b"]);
    assert_eq!(sources(&s)["meeting"]["meetings"], 2);
    assert!(s.meta().get("cleared_at").is_none());
}

#[test]
fn deleted_meeting_drops_its_chunks() {
    let s = alone();
    let (a, b) = (meeting_of("a", "하나"), meeting_of("b", "둘"));
    meeting::save(&s.r.meeting_dir(), &[a.clone(), b]).unwrap();
    s.sync(Target::default());
    meeting::save(&s.r.meeting_dir(), &[a]).unwrap();
    s.sync(Target::default());
    assert_eq!(paths(&s, "meeting"), ["meeting/a"]);
    assert_eq!(sources(&s)["meeting"]["meetings"], 1);
}

#[test]
fn meetings_only_then_empty_returns_to_seed() {
    let s = alone();
    meeting::save(&s.r.meeting_dir(), &[meeting_of("a", "하나")]).unwrap();
    s.sync(Target::default());
    meeting::save(&s.r.meeting_dir(), &[]).unwrap();
    s.sync(Target::default());
    assert!(s.engine.index.load().raw.is_empty());
    meeting::forget(&s.r.meeting_dir()).unwrap();
    s.sync(Target::default());
    assert!(s.engine.index.load().raw.is_empty());
    assert!(s.meta().get("cleared_at").is_some());
    assert!(s.meta().get("sources").is_none());
}

#[test]
fn meetings_beside_repo() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let code = s.engine.index.load().raw.len();
    meeting::save(
        &s.r.meeting_dir(),
        &[meeting_of("a", "카드 결제만 먼저 내요")],
    )
    .unwrap();
    s.sync(Target::default());
    assert_eq!(s.engine.index.load().raw.len(), code + 1);
    assert_eq!(paths(&s, "meeting"), ["meeting/a"]);
    assert!(sources(&s).get("repo").is_some());
    assert_eq!(sources(&s)["meeting"]["meetings"], 1);
}

#[test]
fn meetings_stay_beside_a_failed_github_sync() {
    let s = Sandbox::new();
    s.sync(Target::default());
    meeting::save(
        &s.r.meeting_dir(),
        &[meeting_of("a", "카드 결제만 먼저 내요")],
    )
    .unwrap();
    s.github.set_credentials("none", None);
    let st = s.run(Target::default());
    assert_eq!(st["state"], "failed");
    assert_eq!(paths(&s, "meeting"), ["meeting/a"]);
    assert!(sources(&s).get("repo").is_some());
}

#[test]
fn notion_disconnect_keeps_meetings() {
    let s = alone();
    s.notion_api.pages.lock().unwrap().push((
        "p-1".into(),
        "배포 순서".into(),
        "금요일에는 배포하지 않아요.".into(),
    ));
    s.notion.set(Some("token"), Some("팀"));
    meeting::save(&s.r.meeting_dir(), &[meeting_of("a", "하나")]).unwrap();
    s.sync(Target::default());
    assert_eq!(paths(&s, "notion").len(), 1);
    s.notion.set(None, None);
    notion::forget(&s.r.notion_dir()).unwrap();
    s.sync(Target::default());
    assert!(paths(&s, "notion").is_empty());
    assert_eq!(paths(&s, "meeting"), ["meeting/a"]);
    assert!(s.meta().get("cleared_at").is_none());
}

#[test]
fn notion_failure_builds_from_meetings() {
    let s = alone();
    s.notion.set(Some("token"), Some("팀"));
    s.notion_api.fail.store(true, Ordering::SeqCst);
    meeting::save(&s.r.meeting_dir(), &[meeting_of("a", "하나")]).unwrap();
    let st = s.run(Target::default());
    assert_eq!(st["state"], "done", "{st}");
    assert_eq!(paths(&s, "meeting"), ["meeting/a"]);
}

#[test]
fn clear_keeps_the_meetings_file() {
    let s = alone();
    meeting::save(&s.r.meeting_dir(), &[meeting_of("a", "하나")]).unwrap();
    s.sync(Target::default());
    assert!(s.r.clear());
    assert!(s.r.meeting_dir().join(meeting::FILE).exists());
}

fn post(s: &Sandbox, meetings: &[Meeting]) -> (StatusCode, Value) {
    s.call(
        "POST",
        "/meetings",
        &json!({"meetings": meetings}).to_string(),
    )
}

/// 갱신이 끝날 때까지, 그리고 이어서 걸린 갱신도 끝날 때까지 기다려요.
fn settle(s: &Sandbox) {
    for _ in 0..3 {
        finish(&s.r);
        std::thread::sleep(Duration::from_millis(150));
    }
    finish(&s.r);
}

#[test]
fn meetings_route_saves_and_counts() {
    let s = alone();
    let (code, out) = post(&s, &[meeting_of("a", "하나"), meeting_of("b", "둘")]);
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"count": 2}));
    assert!(s.r.meeting_dir().join(meeting::FILE).exists());
    settle(&s);
    assert_eq!(paths(&s, "meeting"), ["meeting/a", "meeting/b"]);
}

#[test]
fn meetings_route_rejects_bad_body() {
    let s = alone();
    for bad in [
        "{}",
        "nope",
        r#"{"meetings": "x"}"#,
        r#"{"meetings": [{"id": "a"}]}"#,
        r#"{"meetings": [{"id": 1, "title": "t", "start": "s", "end": "e"}]}"#,
    ] {
        let (code, _) = s.call("POST", "/meetings", bad);
        assert_eq!(code, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert!(!s.r.meeting_dir().join(meeting::FILE).exists());
}

#[test]
fn same_meetings_do_not_start_refresh() {
    let s = alone();
    let list = [meeting_of("a", "하나")];
    post(&s, &list);
    settle(&s);
    let before = s.r.status()["started_at"].clone();
    let (code, out) = post(&s, &list);
    assert_eq!(code, StatusCode::OK);
    assert_eq!(out, json!({"count": 1}));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(s.r.status()["started_at"], before);
}

#[test]
fn meetings_push_during_refresh_runs_again() {
    let s = alone();
    s.linear_api.issue(
        "1",
        "t1",
        "배포 순서",
        "금요일에는 배포하지 않아요.",
        "2026-10-30T00:00:00.000Z",
    );
    s.linear
        .set(Some("token".into()), Some("팀".into()), Some("w1".into()));
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    *s.linear_api.gate.lock().unwrap() = Some((entered_tx, release_rx));
    assert!(s.r.start(Target::default()));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    let (code, _) = post(&s, &[meeting_of("a", "카드 결제만 먼저 내요")]);
    assert_eq!(code, StatusCode::OK);
    release.send(()).unwrap();
    settle(&s);
    assert_eq!(paths(&s, "meeting"), ["meeting/a"]);
    assert!(s.texts().contains("금요일에는 배포하지 않아요."));
}

#[test]
fn meetings_push_beside_repo_asks_github_nothing() {
    let s = Sandbox::new();
    s.sync(Target::default());
    let code = s.engine.index.load().raw.len();
    let calls = s.calls().len();
    let before = s.r.status();
    post(&s, &[meeting_of("a", "카드 결제만 먼저 내요")]);
    settle(&s);
    assert_eq!(paths(&s, "meeting"), ["meeting/a"]);
    assert_eq!(s.engine.index.load().raw.len(), code + 1);
    assert_eq!(sources(&s)["meeting"]["meetings"], 1);
    assert_eq!(
        s.calls().len(),
        calls,
        "no GitHub request for a meetings change"
    );
    assert_eq!(s.r.status(), before, "the last sync's status stays");
    post(&s, &[]);
    settle(&s);
    assert!(paths(&s, "meeting").is_empty());
    assert_eq!(s.engine.index.load().raw.len(), code);
    assert_eq!(s.calls().len(), calls);
}

#[test]
fn meetings_push_keeps_cleared_data_cleared() {
    let s = Sandbox::new();
    s.sync(Target::default());
    assert!(s.r.clear());
    let calls = s.calls().len();
    post(&s, &[meeting_of("a", "하나")]);
    settle(&s);
    assert!(s.engine.index.load().raw.is_empty());
    assert!(s.meta().get("cleared_at").is_some());
    assert_eq!(s.calls().len(), calls);
}

#[test]
fn first_empty_push_leaves_the_seed() {
    let s = alone();
    let before = s.engine.index.load().raw.len();
    post(&s, &[]);
    settle(&s);
    assert_eq!(s.engine.index.load().raw.len(), before);
    assert!(s.meta().get("cleared_at").is_some());
    assert!(s.meta().get("sources").is_none());
}

#[test]
fn saved_but_unindexed_meetings_go_in_on_the_next_push() {
    let s = alone();
    // saved while the search model was still on its way: the file has it, the index does not
    meeting::save(&s.r.meeting_dir(), &[meeting_of("a", "하나")]).unwrap();
    post(&s, &[meeting_of("a", "하나")]);
    settle(&s);
    assert_eq!(paths(&s, "meeting"), ["meeting/a"]);
    // a seed index holds nothing else: only the meeting is searched
    assert_eq!(s.engine.index.load().raw.len(), 1);
    assert!(s.meta().get("cleared_at").is_none());
    assert_eq!(sources(&s)["meeting"]["meetings"], 1);
}

#[test]
fn concurrent_saves_leave_one_whole_list() {
    let s = alone();
    let dir = s.r.meeting_dir();
    let lists: Vec<Vec<Meeting>> = (0..8)
        .map(|i| {
            (0..=i)
                .map(|j| meeting_of(&format!("m{j}"), "결정"))
                .collect()
        })
        .collect();
    let handles: Vec<_> = lists
        .iter()
        .cloned()
        .map(|list| {
            let dir = dir.clone();
            std::thread::spawn(move || meeting::save(&dir, &list).map(|_| ()))
        })
        .collect();
    for h in handles {
        h.join().unwrap().unwrap();
    }
    let (chunks, info) = meeting::gather(&dir);
    let n = info.unwrap()["meetings"].as_u64().unwrap() as usize;
    assert_eq!(chunks.len(), n);
    assert!((1..=8).contains(&n));
}

fn entry(kind: &str, text: &str, done: bool) -> Entry {
    Entry {
        kind: kind.into(),
        text: text.into(),
        owner: Some("민호".into()),
        due: None,
        done,
        replaces: None,
    }
}

#[test]
fn load_returns_saved_meetings_in_asked_order_and_skips_missing() {
    let dir = tempfile::tempdir().unwrap();
    let all = vec![
        meeting_of("a", "A"),
        meeting_of("b", "B"),
        meeting_of("c", "C"),
    ];
    meeting::save(dir.path(), &all).unwrap();
    let ids: Vec<String> = ["c", "zz", "a"].iter().map(|s| s.to_string()).collect();
    let got = meeting::load(dir.path(), &ids);
    let got_ids: Vec<&str> = got.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(got_ids, ["c", "a"]);
    assert!(meeting::load(tempfile::tempdir().unwrap().path(), &ids).is_empty());
}

#[test]
fn open_items_keeps_open_questions_and_unfinished_commitments() {
    let mut m1 = meeting_of("a", "x");
    m1.ledger = vec![
        entry("decision", "결정", false),
        entry("commitment", "끝난 일", true),
        entry("commitment", "할 일", false),
    ];
    let mut m2 = meeting_of("b", "y");
    m2.ledger = vec![entry("open_question", "질문", false)];
    let items = meeting::open_items(&[m1, m2]);
    let got: Vec<(&str, usize, &str)> = items
        .iter()
        .map(|i| (i.meeting_id.as_str(), i.index, i.text.as_str()))
        .collect();
    assert_eq!(got, [("a", 2, "할 일"), ("b", 0, "질문")]);
    assert_eq!(items[0].owner.as_deref(), Some("민호"));
}

/// 답 모델은 회의 조각의 날짜를 봐야 같은 일을 다르게 정한 회의 중 나중 결정을 골라요(docs/30 5절 3).
#[test]
fn meeting_passage_keeps_its_date_for_the_answer_model() {
    use tacie_engine::cards::split_chunk;
    use tacie_engine::pipeline::passage_text;
    let (head, body) = split_chunk("meeting: 결제 화면\nmeta: 회의 · 2026-11-03\n결정: 재시도는 3번");
    assert_eq!(
        passage_text("meeting", &head, &body),
        "회의 · 2026-11-03\n결정: 재시도는 3번"
    );
    let (head, body) = split_chunk("slack: #dev\nmeta: 지윤 · 2026-11-03\n재시도는 3번");
    assert_eq!(passage_text("slack", &head, &body), "재시도는 3번");
}
