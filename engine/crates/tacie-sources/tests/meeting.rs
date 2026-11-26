//! The past meetings source (docs/30): saves meetings the app sent and turns each meeting into one document.

use serde_json::Value;
use tacie_sources::meeting::{Entry, Meeting, Topic, doc_of, forget, gather, save};

fn meeting(id: &str, summary: &[&str], ledger: Vec<Entry>) -> Meeting {
    Meeting {
        id: id.to_string(),
        title: "결제 화면 정리".to_string(),
        start: "2026-11-03T14:02:10+09:00".to_string(),
        end: "2026-11-03T14:41:55+09:00".to_string(),
        summary: summary.iter().map(|s| s.to_string()).collect(),
        topics: vec![Topic {
            title: "결제 버튼".to_string(),
            points: vec!["버튼을 아래로 옮겨요".to_string()],
        }],
        ledger,
    }
}

fn entry(kind: &str, text: &str, owner: Option<&str>, due: Option<&str>, done: bool) -> Entry {
    Entry {
        kind: kind.to_string(),
        text: text.to_string(),
        owner: owner.map(str::to_string),
        due: due.map(str::to_string),
        done,
        replaces: None,
    }
}

fn lines(m: &Meeting) -> Vec<String> {
    doc_of(m).body.into_iter().map(|(l, _)| l).collect()
}

#[test]
fn doc_has_path_date_and_korean_labels() {
    let m = meeting(
        "20261103-140210",
        &["결제 화면을 한 장으로 줄이기로 했어요"],
        vec![
            entry("decision", "카드 결제만 먼저 내요", None, None, false),
            entry(
                "commitment",
                "시안을 다시 그려요",
                Some("지윤"),
                Some("금요일까지"),
                true,
            ),
            entry("open_question", "환불은 어디서 하죠?", None, None, false),
        ],
    );
    let d = doc_of(&m);
    assert_eq!(d.path, "meeting/20261103-140210");
    assert_eq!(d.title, "결제 화면 정리");
    assert_eq!(d.at, "2026-11-03");
    assert_eq!(d.url, "");
    assert_eq!(d.acl, Some(vec!["*".to_string()]));
    assert_eq!(d.head, "meeting: 결제 화면 정리\nmeta: 회의 · 2026-11-03");
    assert_eq!(
        lines(&m),
        vec![
            "결제 화면을 한 장으로 줄이기로 했어요",
            "결정: 카드 결제만 먼저 내요",
            "할 일: 시안을 다시 그려요 (지윤, 금요일까지) · 끝냄",
            "열린 질문: 환불은 어디서 하죠?",
            "주제: 결제 버튼",
            "- 버튼을 아래로 옮겨요",
        ]
    );
}

#[test]
fn english_meeting_gets_english_labels() {
    let mut m = meeting(
        "20261104-090000",
        &["We agreed to ship card payments first and leave refunds for later."],
        vec![
            entry("decision", "Ship card payments first", None, None, false),
            entry(
                "commitment",
                "Redraw the checkout",
                Some("Speaker 2"),
                None,
                false,
            ),
        ],
    );
    m.title = "Checkout sync".to_string();
    m.topics = vec![Topic {
        title: "Checkout".to_string(),
        points: vec!["Move the button down".to_string()],
    }];
    let d = doc_of(&m);
    assert_eq!(d.head, "meeting: Checkout sync\nmeta: Meeting · 2026-11-03");
    let ls = lines(&m);
    assert!(ls.contains(&"Decision: Ship card payments first".to_string()));
    assert!(ls.contains(&"To-do: Redraw the checkout (Speaker 2)".to_string()));
    assert!(ls.contains(&"Topic: Checkout".to_string()));
}

#[test]
fn save_reports_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let a = meeting("20261103-140210", &["하나"], vec![]);
    let b = meeting("20261104-100000", &["둘"], vec![]);
    assert!(save(dir.path(), &[a.clone(), b.clone()]).unwrap());
    assert!(!save(dir.path(), &[a.clone(), b]).unwrap());
    assert!(save(dir.path(), &[a]).unwrap());
}

#[test]
fn gather_without_file_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (chunks, info) = gather(dir.path());
    assert!(chunks.is_empty());
    assert!(info.is_none());
}

#[test]
fn gather_counts_meetings_and_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let a = meeting("20261103-140210", &["하나"], vec![]);
    let b = meeting("20261104-100000", &["둘"], vec![]);
    save(dir.path(), &[a, b]).unwrap();
    let (chunks, info) = gather(dir.path());
    assert_eq!(chunks.len(), 2);
    assert!(chunks.iter().all(|c| c["src"] == "meeting"));
    let info = info.unwrap();
    assert_eq!(info["meetings"], 2);
    assert_eq!(info["chunks"], 2);
    assert!(info["saved_at"].is_string());
}

#[test]
fn empty_list_gives_no_chunks_but_info() {
    let dir = tempfile::tempdir().unwrap();
    save(dir.path(), &[]).unwrap();
    let (chunks, info) = gather(dir.path());
    assert!(chunks.is_empty());
    assert_eq!(info.unwrap()["meetings"], 0);
}

#[test]
fn forget_removes_and_tolerates_missing() {
    let dir = tempfile::tempdir().unwrap();
    forget(dir.path()).unwrap();
    save(dir.path(), &[meeting("20261103-140210", &["하나"], vec![])]).unwrap();
    forget(dir.path()).unwrap();
    assert!(gather(dir.path()).1.is_none());
}

#[test]
fn long_meeting_windows_share_path() {
    let many: Vec<String> = (0..60).map(|i| format!("요약 줄 {i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let dir = tempfile::tempdir().unwrap();
    save(dir.path(), &[meeting("20261103-140210", &refs, vec![])]).unwrap();
    let (chunks, _) = gather(dir.path());
    assert!(chunks.len() >= 2);
    for c in &chunks {
        assert_eq!(c["path"], Value::from("meeting/20261103-140210"));
        assert_eq!(c["src"], Value::from("meeting"));
    }
}

#[test]
fn changed_decision_carries_the_earlier_one_into_the_search_text() {
    let mut e = entry("decision", "재시도를 5번으로 늘림", None, None, false);
    e.replaces = Some(tacie_sources::meeting::Replaced {
        text: "재시도는 3번까지".into(),
        at: "2026-10-30".into(),
        title: "결제 정리".into(),
    });
    let m = Meeting {
        id: "a".into(),
        title: "결제".into(),
        start: "2026-11-12T10:00:00+09:00".into(),
        end: "2026-11-12T10:30:00+09:00".into(),
        summary: vec![],
        topics: vec![],
        ledger: vec![e],
    };
    assert_eq!(
        lines(&m),
        vec!["결정: 재시도를 5번으로 늘림 (이전 결정 2026-10-30 결제 정리: 재시도는 3번까지)"]
    );
}
