//! Turns ADF (Atlassian Document Format) into Markdown text.
//!
//! Jira v3 returns ADF as an object; Confluence v2's `body.atlas_doc_format.value` is a JSON **string** holding ADF.
//! Unknown nodes keep only their inner text; images, attachments (media) and emoji have no text and are dropped.

use chrono::DateTime;
use serde_json::Value;

/// 객체 또는 JSON 문자열로 온 ADF를 객체로 돌려줘요. 읽을 수 없으면 `None`이에요.
pub fn parse_doc(v: &Value) -> Option<Value> {
    match v {
        Value::Object(_) => Some(v.clone()),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(d @ Value::Object(_)) => Some(d),
            _ => None,
        },
        _ => None,
    }
}

/// ADF 문서를 Markdown으로 바꿔요. 블록은 빈 줄로 이어요.
pub fn to_markdown(doc: &Value) -> String {
    blocks(doc)
}

/// 글에 나온 멘션을 (accountId, `@` 뺀 표시 글) 순서대로 모아요.
pub fn mentions(doc: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect_mentions(doc, &mut out);
    out
}

fn collect_mentions(node: &Value, out: &mut Vec<(String, String)>) {
    if node.get("type").and_then(Value::as_str) == Some("mention") {
        let id = attr(node, "id");
        if !id.is_empty() {
            out.push((
                id.to_string(),
                attr(node, "text").trim_start_matches('@').to_string(),
            ));
        }
    }
    for child in children(node) {
        collect_mentions(child, out);
    }
}

fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn attr<'a>(node: &'a Value, key: &str) -> &'a str {
    node.get("attrs")
        .and_then(|a| a.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn kind(node: &Value) -> &str {
    node.get("type").and_then(Value::as_str).unwrap_or("")
}

/// 자식 블록을 하나씩 바꿔 빈 줄로 이어요. 빈 블록은 건너뛰어요.
fn blocks(node: &Value) -> String {
    children(node)
        .iter()
        .map(block)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block(node: &Value) -> String {
    match kind(node) {
        "paragraph" => inline(node),
        "heading" => {
            let level = node
                .get("attrs")
                .and_then(|a| a.get("level"))
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6);
            let text = inline(node);
            if text.is_empty() {
                String::new()
            } else {
                format!("{} {}", "#".repeat(level as usize), text)
            }
        }
        "bulletList" => list(node, false),
        "orderedList" => list(node, true),
        "codeBlock" => {
            let lang = attr(node, "language");
            format!("```{lang}\n{}\n```", inline(node))
        }
        "blockquote" => quote(&blocks(node), ""),
        "panel" => quote(&blocks(node), "참고: "),
        "table" => table(node),
        "rule" => "---".to_string(),
        "mediaSingle" | "mediaGroup" | "media" => String::new(),
        // 모르는 블록: 안의 글만 살려요.
        _ => {
            let kids = children(node);
            if kids.iter().any(is_inline) {
                inline(node)
            } else {
                blocks(node)
            }
        }
    }
}

fn is_inline(node: &Value) -> bool {
    matches!(
        kind(node),
        "text"
            | "hardBreak"
            | "mention"
            | "status"
            | "date"
            | "emoji"
            | "inlineCard"
            | "mediaInline"
    )
}

fn quote(body: &str, first_prefix: &str) -> String {
    body.lines()
        .enumerate()
        .map(|(i, l)| {
            let p = if i == 0 { first_prefix } else { "" };
            if l.is_empty() && p.is_empty() {
                ">".to_string()
            } else {
                format!("> {p}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn list(node: &Value, ordered: bool) -> String {
    let mut lines = Vec::new();
    for (i, item) in children(node).iter().enumerate() {
        let marker = if ordered {
            format!("{}. ", i + 1)
        } else {
            "- ".to_string()
        };
        let mut parts: Vec<String> = Vec::new();
        for child in children(item) {
            let s = block(child);
            if !s.is_empty() {
                parts.push(s);
            }
        }
        let body = parts.join("\n");
        let indent = " ".repeat(2.max(marker.len()));
        let mut it = body.lines();
        let first = it.next().unwrap_or("");
        lines.push(format!("{marker}{first}"));
        for l in it {
            lines.push(if l.is_empty() {
                String::new()
            } else {
                format!("{indent}{l}")
            });
        }
    }
    lines.join("\n")
}

fn table(node: &Value) -> String {
    let mut rows = Vec::new();
    for row in children(node) {
        let cells: Vec<String> = children(row)
            .iter()
            .map(|c| blocks(c).replace('|', "\\|").replace('\n', " "))
            .collect();
        rows.push(format!("| {} |", cells.join(" | ")));
        if rows.len() == 1 {
            rows.push(format!("|{}", " --- |".repeat(cells.len().max(1))));
        }
    }
    rows.join("\n")
}

fn inline(node: &Value) -> String {
    children(node).iter().map(inline_node).collect()
}

fn inline_node(n: &Value) -> String {
    match kind(n) {
        "text" => {
            let text = n.get("text").and_then(Value::as_str).unwrap_or("");
            let link = n
                .get("marks")
                .and_then(Value::as_array)
                .and_then(|ms| ms.iter().find(|m| kind(m) == "link"))
                .map(|m| attr(m, "href"))
                .filter(|h| !h.is_empty());
            match link {
                Some(h) if h != text => format!("{text} ({h})"),
                _ => text.to_string(),
            }
        }
        "hardBreak" => "\n".to_string(),
        "mention" => attr(n, "text").to_string(),
        "status" => format!("[{}]", attr(n, "text")),
        "date" => attr(n, "timestamp")
            .parse::<i64>()
            .ok()
            .and_then(DateTime::from_timestamp_millis)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default(),
        "inlineCard" => attr(n, "url").to_string(),
        "emoji" | "mediaInline" => String::new(),
        _ => inline(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t(s: &str) -> Value {
        json!({"type":"text","text":s})
    }
    fn p(s: &str) -> Value {
        json!({"type":"paragraph","content":[t(s)]})
    }
    fn doc(c: Vec<Value>) -> Value {
        json!({"type":"doc","version":1,"content":c})
    }

    #[test]
    fn paragraphs_headings_lists_become_markdown() {
        let d = doc(vec![
            json!({"type":"heading","attrs":{"level":2},"content":[t("결정")]}),
            p("Payfold를 쓰기로 했어요."),
            json!({"type":"bulletList","content":[
                {"type":"listItem","content":[p("하나")]},
                {"type":"listItem","content":[p("둘")]}]}),
        ]);
        assert_eq!(
            to_markdown(&d),
            "## 결정\n\nPayfold를 쓰기로 했어요.\n\n- 하나\n- 둘"
        );
    }

    #[test]
    fn nested_and_ordered_lists_indent() {
        let d = doc(vec![json!({"type":"orderedList","content":[
            {"type":"listItem","content":[p("a"),
                {"type":"bulletList","content":[{"type":"listItem","content":[p("b")]}]}]},
            {"type":"listItem","content":[p("c")]}]})]);
        assert_eq!(to_markdown(&d), "1. a\n   - b\n2. c");
    }

    #[test]
    fn tables_become_pipe_rows() {
        let cell = |k: &str, s: &str| json!({"type":k,"content":[p(s)]});
        let d = doc(vec![json!({"type":"table","content":[
            {"type":"tableRow","content":[cell("tableHeader","후보"),cell("tableHeader","이유")]},
            {"type":"tableRow","content":[cell("tableCell","Payfold"),cell("tableCell","계약 필요")]}]})]);
        let md = to_markdown(&d);
        assert!(md.contains("| 후보 | 이유 |"), "{md}");
        assert!(md.contains("| Payfold | 계약 필요 |"), "{md}");
    }

    #[test]
    fn marks_links_mentions_status_dates() {
        let d = doc(vec![json!({"type":"paragraph","content":[
            {"type":"text","text":"굵게","marks":[{"type":"strong"}]},
            t(" "),
            {"type":"text","text":"글자","marks":[{"type":"link","attrs":{"href":"https://x"}}]},
            t(" "),
            {"type":"mention","attrs":{"id":"abc","text":"@지윤"}},
            t(" "),
            {"type":"status","attrs":{"text":"진행 중","color":"blue"}},
            t(" "),
            {"type":"date","attrs":{"timestamp":"1767225600000"}}]})]);
        assert_eq!(
            to_markdown(&d),
            "굵게 글자 (https://x) @지윤 [진행 중] 2026-01-01"
        );
    }

    #[test]
    fn code_quote_panel() {
        let d = doc(vec![
            json!({"type":"codeBlock","attrs":{"language":"rust"},"content":[t("let a = 1;")]}),
            json!({"type":"blockquote","content":[p("인용")]}),
            json!({"type":"panel","attrs":{"panelType":"info"},"content":[p("메모")]}),
        ]);
        let md = to_markdown(&d);
        assert!(md.contains("```rust\nlet a = 1;\n```"), "{md}");
        assert!(md.contains("> 인용"), "{md}");
        assert!(md.contains("> 참고: 메모"), "{md}");
    }

    #[test]
    fn unknown_nodes_keep_their_text_and_media_is_dropped() {
        let d = doc(vec![
            json!({"type":"mystery","content":[p("안")]}),
            json!({"type":"mediaSingle","content":[{"type":"media","attrs":{"id":"m"}}]}),
            json!({"type":"paragraph","content":[{"type":"emoji","attrs":{"shortName":":smile:"}}]}),
        ]);
        assert_eq!(to_markdown(&d), "안");
    }

    #[test]
    fn confluence_string_value_is_parsed() {
        assert!(parse_doc(&json!("{\"type\":\"doc\",\"content\":[]}")).is_some());
        assert!(parse_doc(&json!("not json")).is_none());
        assert!(parse_doc(&json!({"type":"doc"})).is_some());
    }

    #[test]
    fn mentions_are_collected() {
        let d = doc(vec![json!({"type":"paragraph","content":[
            {"type":"mention","attrs":{"id":"abc","text":"@지윤"}},
            {"type":"mention","attrs":{"id":"def","text":"@민호"}}]})]);
        assert_eq!(
            mentions(&d),
            vec![
                ("abc".to_string(), "지윤".to_string()),
                ("def".to_string(), "민호".to_string())
            ]
        );
    }
}
