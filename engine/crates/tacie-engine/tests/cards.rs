use std::path::Path;

use serde_json::{Map, Value, json};
use tacie_engine::cards::{build_cards, card, split_chunk, title_of};
use tacie_index::Chunk;

fn chunk(src: &str, path: &str, line: u64, text: &str) -> Chunk {
    Chunk {
        src: src.into(),
        path: path.into(),
        line,
        text: text.into(),
        title: None,
        url: None,
        acl: None,
        extra: Map::new(),
    }
}

fn chunks() -> Vec<Chunk> {
    vec![
        chunk(
            "code",
            "apps/api/src/mail/session.ts",
            41,
            "file: apps/api/src/mail/session.ts\nsymbols: createSession, refresh\nexport function createSession() {\n  return 1;\n}",
        ),
        chunk(
            "code",
            "apps/api/src/mail/session.ts",
            71,
            "file: apps/api/src/mail/session.ts\nexport function refresh() {}",
        ),
        chunk(
            "docs",
            "docs/메일/03_로그인.md",
            10,
            "doc: docs/메일/03_로그인.md\nsection: 메일 > 401이 날 때\n## 401이 날 때\n기동 순서를 확인해요.",
        ),
        chunk(
            "code",
            "apps/web/src/mail/Tab.tsx",
            1,
            "file: apps/web/src/mail/Tab.tsx\nexport const Tab = 1;",
        ),
    ]
}

fn meta() -> Map<String, Value> {
    json!({"repo": "acme/widgets", "commit": "abc123"})
        .as_object()
        .unwrap()
        .clone()
}

#[test]
fn split_chunk_strips_header() {
    let cs = chunks();
    let (head, body) = split_chunk(&cs[2].text);
    assert_eq!(head.len(), 2);
    assert_eq!(head["doc"], "docs/메일/03_로그인.md");
    assert_eq!(head["section"], "메일 > 401이 날 때");
    assert_eq!(body, vec!["## 401이 날 때", "기동 순서를 확인해요."]);
}

#[test]
fn build_cards_one_card_per_file_with_pinned_link() {
    let cs = chunks();
    let cards = build_cards(
        &cs,
        &[1, 0, 2, 3],
        &[Some(0.9), Some(0.8), Some(0.7), Some(0.6)],
        &meta(),
        3,
        Some(Path::new("/r")),
    );
    let paths: Vec<&str> = cards.iter().map(|c| c["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        [
            "apps/api/src/mail/session.ts",
            "docs/메일/03_로그인.md",
            "apps/web/src/mail/Tab.tsx"
        ]
    );
    assert_eq!(cards[0]["line_start"], 71);
    assert_eq!(cards[0]["score"], 0.9);
    assert_eq!(
        cards[0]["github_url"],
        "https://github.com/acme/widgets/blob/abc123/apps/api/src/mail/session.ts#L71-L71"
    );
    assert_eq!(cards[1]["title"], "03_로그인.md · 401이 날 때");
    assert_eq!(cards[1]["local_path"], "/r/docs/메일/03_로그인.md");
}

#[test]
fn card_has_python_key_order_and_fields() {
    let cs = chunks();
    let c = card(&cs[2], None, &Map::new(), None, 2);
    let keys: Vec<&str> = c.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "id",
            "path",
            "kind",
            "source",
            "line_start",
            "line_end",
            "title",
            "byline",
            "snippet",
            "score",
            "url",
            "github_url",
            "local_path",
            "chunk"
        ]
    );
    assert_eq!(c["id"].as_str().unwrap().len(), 10);
    assert!(c["id"].as_str().unwrap().starts_with("c_"));
    assert_eq!(c["line_end"], 11);
    assert_eq!(c["snippet"], "## 401이 날 때\n기동 순서를 확인해요.");
    assert_eq!(c["source"], "repo");
    assert_eq!(c["byline"], Value::Null);
    assert_eq!(c["score"], Value::Null);
    assert_eq!(c["url"], Value::Null);
    assert_eq!(c["local_path"], Value::Null);
    assert_eq!(c["chunk"], 2);
}

#[test]
fn title_of_uses_section_for_docs_and_symbol_for_code() {
    let cs = chunks();
    let (h, _) = split_chunk(&cs[0].text);
    assert_eq!(title_of(&cs[0], &h), "session.ts · createSession");
    let (h, _) = split_chunk(&cs[1].text);
    assert_eq!(title_of(&cs[1], &h), "session.ts");
    let (h, _) = split_chunk(&cs[2].text);
    assert_eq!(title_of(&cs[2], &h), "03_로그인.md · 401이 날 때");
    let mut titled = cs[0].clone();
    titled.title = Some("PR #7".into());
    assert_eq!(title_of(&titled, &h), "PR #7");
}

#[test]
fn byline_joins_slack_and_meta_headers() {
    let mut pr = chunk(
        "github",
        "pr/7",
        1,
        "github: basejb/tacie#7\nmeta: 머지됨 · basejb\n본문\n\n",
    );
    pr.url = Some("https://github.com/basejb/tacie/pull/7".into());
    let c = card(&pr, Some(0.5), &meta(), Some(Path::new("/r")), 0);
    assert_eq!(c["byline"], "머지됨 · basejb");
    assert_eq!(c["source"], "github");
    assert_eq!(c["url"], "https://github.com/basejb/tacie/pull/7");
    assert_eq!(c["github_url"], c["url"]);
    assert_eq!(c["local_path"], Value::Null);
    assert_eq!(c["snippet"], "본문");

    let sl = chunk(
        "slack",
        "s/1",
        1,
        "slack: #dev-help · 2026-10-21\nmeta: 스레드\nhi",
    );
    let c = card(&sl, None, &meta(), None, 1);
    assert_eq!(c["byline"], "#dev-help · 2026-10-21 · 스레드");
    assert_eq!(c["github_url"], Value::Null);
    let none = chunk("slack", "s/2", 1, "hi");
    assert_eq!(card(&none, None, &meta(), None, 1)["byline"], "");
}

#[test]
fn linear_byline_is_its_meta_line() {
    let mut li = chunk(
        "linear",
        "linear/issue/1",
        1,
        "linear: COR-7 · 로그인 · Core · 담당 Ann\nmeta: Linear · COR-7 · 완료 · 10월 27일\n본문\n",
    );
    li.url = Some("https://linear.app/a/issue/COR-7#comment-c1".into());
    let c = card(&li, Some(0.9), &meta(), None, 0);
    assert_eq!(c["byline"], "Linear · COR-7 · 완료 · 10월 27일");
    assert_eq!(c["source"], "linear");
    assert_eq!(c["snippet"], "본문");
    assert_eq!(c["url"], "https://linear.app/a/issue/COR-7#comment-c1");
}

#[test]
fn dropbox_byline_is_its_meta_line() {
    let mut c = chunk(
        "dropbox",
        "dropbox/p1",
        1,
        "dropbox: 회의록.paper\nmeta: Dropbox · 영업 / 제안서 · 10월 27일 수정\n결정\n",
    );
    c.url = Some("https://www.dropbox.com/home/%EC%98%81%EC%97%85?preview=a.paper".into());
    let card = card(&c, Some(0.9), &meta(), None, 0);
    assert_eq!(card["byline"], "Dropbox · 영업 / 제안서 · 10월 27일 수정");
    assert_eq!(card["source"], "dropbox");
    assert_eq!(card["snippet"], "결정");
    assert_eq!(
        card["url"],
        "https://www.dropbox.com/home/%EC%98%81%EC%97%85?preview=a.paper"
    );
}

#[test]
fn meeting_card_hides_header_and_has_byline() {
    let mut m = chunk(
        "meeting",
        "meeting/20261103-140210",
        1,
        "meeting: 결제 화면 정리\nmeta: 회의 · 2026-11-03\n결정: 카드 결제만 먼저 내요",
    );
    m.title = Some("결제 화면 정리".into());
    m.url = Some(String::new());
    let c = card(&m, Some(0.9), &meta(), None, 0);
    assert_eq!(c["kind"], "meeting");
    assert_eq!(c["source"], "meeting");
    assert_eq!(c["title"], "결제 화면 정리");
    assert_eq!(c["byline"], "회의 · 2026-11-03");
    assert_eq!(c["snippet"], "결정: 카드 결제만 먼저 내요");
    assert_eq!(c["path"], "meeting/20261103-140210");
    assert_eq!(c["url"], "");
    assert_eq!(c["github_url"], Value::Null);
    assert_eq!(c["local_path"], Value::Null);
}
