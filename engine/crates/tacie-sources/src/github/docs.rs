//! Builds documents and chunks from GitHub issues and PRs. Document and chunk text must not change (if even one
//! character changes, every chunk is embedded again).
//!
//! One issue or PR is one document: a header (title, state, author, branch or labels), then at the very start of the
//! body the changed files (the first 8, tests excluded), the description, and conversation, review summaries and
//! inline review comments in time order. Posts by bot accounts are left out (CI and coverage noise).
//!
//! An item with an odd shape is an `InvalidData` error holding only the number and key name (no title or body). It is
//! also an error when a required key is missing, when `html_url`/`url` is null or not a string, when the author login
//! is not a string, or when `totalCount` is not an integer (document fields are strings and integers). The GitHub
//! schema never sends such values.
//!
//! The user's own unsubmitted review (PENDING) comes from GraphQL as `submittedAt: null`. So such a review does not
//! block the whole refresh, events with a None time go first and it carries on (`Option` ordering). When two events
//! with the same time and title have bodies None and a string, they are sorted the same way and it carries on.

use std::io::{Error, ErrorKind};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use tacie_index::{RawChunk, pyjson};

use super::items::load_items;
use crate::doc::{Doc, windows};
use crate::py;

static TESTS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(__tests__|\.test\.|\.spec\.)").unwrap());
static COMMENTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?mi)(!\[[^\]]*\]\([^)]*\)|^.*Generated with \[?Claude Code.*$|^Co-Authored-By:.*$)",
    )
    .unwrap()
});
static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// 머리 다음 본문에 보이는 바뀐 파일 수예요(Jev의 900자 안에 머리가 짧게 들어가게요).
const SHOWN_FILES: usize = 8;

/// 글에서 HTML 주석, 그림, Claude Code 서명 줄, Co-Authored-By 줄을 빼고, CRLF를 LF로, 빈 줄 셋 이상을 둘로 줄이고,
/// 앞뒤 공백을 빼요.
pub fn clean(text: &str) -> String {
    let text = COMMENTS.replace_all(text, "");
    let text = NOISE.replace_all(&text, "").replace("\r\n", "\n");
    let text = BLANKS.replace_all(&text, "\n\n");
    py::strip(&text).to_string()
}

/// 이슈와 PR을 문서로, 번호 큰 것부터예요. 봇이 연 것은 빠져요. 읽을 수 없는 원본 파일은 건너뛰어요(`load_items`).
pub fn documents(raw: &Path) -> std::io::Result<Vec<Doc>> {
    let (items, _) = load_items(raw)?;
    let mut out = Vec::new();
    for (&n, item) in items.iter().rev() {
        if let Some(doc) = Item(n).document(item)? {
            out.push(doc);
        }
    }
    Ok(out)
}

/// 문서들의 조각이에요(`src` = "github").
pub fn chunks(raw: &Path) -> std::io::Result<Vec<RawChunk>> {
    Ok(documents(raw)?
        .iter()
        .flat_map(|d| windows(d, "github"))
        .collect())
}

/// 본문에 놓을 사건 하나예요: (시각, 제목 줄, 글, URL). 이 순서로 정렬해요.
type Event = (Option<String>, String, Option<String>, String);

static EMPTY: LazyLock<Map<String, Value>> = LazyLock::new(Map::new);

/// 오류에 번호만 담으려고 들고 다녀요.
#[derive(Clone, Copy)]
struct Item(u64);

impl Item {
    fn bad(self, what: &str) -> Error {
        Error::new(
            ErrorKind::InvalidData,
            format!("github: item #{} has a bad {what}", self.0),
        )
    }

    /// obj가 객체가 아니거나 키가 없으면 오류예요.
    fn key<'a>(self, obj: &'a Value, key: &str, what: &str) -> std::io::Result<&'a Value> {
        obj.as_object()
            .and_then(|m| m.get(key))
            .ok_or_else(|| self.bad(what))
    }

    /// 글이어야 하는 값이에요. 글이 아니면 오류예요.
    fn text<'a>(self, v: &'a Value, what: &str) -> std::io::Result<&'a str> {
        v.as_str().ok_or_else(|| self.bad(what))
    }

    /// 머리 줄에 찍는 값이에요: 글은 그대로, null은 `None`, 참·거짓은 `True`/`False`, 수는 JSON 표기예요(조각 글이
    /// 바뀌지 않게 이 표기를 지켜요).
    fn shown(self, v: &Value, what: &str) -> std::io::Result<String> {
        match v {
            Value::String(s) => Ok(s.clone()),
            Value::Null => Ok("None".to_string()),
            Value::Bool(true) => Ok("True".to_string()),
            Value::Bool(false) => Ok("False".to_string()),
            Value::Number(_) => Ok(pyjson::to_string(v, None)),
            _ => Err(self.bad(what)),
        }
    }

    /// 시각의 앞 10글자(날짜)예요. 거짓 값이면 빈 글이에요.
    fn date(self, v: &Value, what: &str) -> std::io::Result<String> {
        if !py::truthy(v) {
            return Ok(String::new());
        }
        Ok(py::take_chars(self.text(v, what)?, 10).to_string())
    }

    /// 정렬 열쇠로 쓰는 값이에요(시각, 글): 거짓 값은 None, 글은 그대로, 그 밖은 오류예요.
    fn sort_key(self, v: &Value, what: &str) -> std::io::Result<Option<String>> {
        if !py::truthy(v) {
            return Ok(None);
        }
        Ok(Some(self.text(v, what)?.to_string()))
    }

    /// 거짓 값이면 빈 글로 보고 `clean`해요.
    fn cleaned(self, v: Option<&Value>, what: &str) -> std::io::Result<String> {
        match v {
            Some(v) if py::truthy(v) => Ok(clean(self.text(v, what)?)),
            _ => Ok(String::new()),
        }
    }

    /// 거짓 값이면 빈 객체, 객체가 아닌 참 값은 오류예요.
    fn object_or_empty<'a>(
        self,
        v: Option<&'a Value>,
        what: &str,
    ) -> std::io::Result<&'a Map<String, Value>> {
        match v {
            Some(v) if py::truthy(v) => v.as_object().ok_or_else(|| self.bad(what)),
            _ => Ok(&EMPTY),
        }
    }

    /// 거짓 값이면 빈 목록, 목록이 아닌 참 값은 오류예요.
    fn list_or_empty<'a>(self, v: Option<&'a Value>, what: &str) -> std::io::Result<&'a [Value]> {
        match v {
            Some(v) if py::truthy(v) => v
                .as_array()
                .map(Vec::as_slice)
                .ok_or_else(|| self.bad(what)),
            _ => Ok(&[]),
        }
    }

    /// 봇 계정이에요: 로그인이 `[bot]`으로 끝나거나 종류가 Bot이에요.
    fn bot(self, user: Option<&Value>, what: &str) -> std::io::Result<bool> {
        let user = self.object_or_empty(user, what)?;
        let login = match user.get("login") {
            Some(v) if py::truthy(v) => self.text(v, what)?,
            _ => "",
        };
        Ok(login.ends_with("[bot]") || user.get("type").and_then(Value::as_str) == Some("Bot"))
    }

    /// 항목 하나를 문서로예요. 봇이 연 것이면 None이에요.
    fn document(self, item: &Value) -> std::io::Result<Option<Doc>> {
        let it = self.key(item, "issue", "issue")?;
        if self.bot(it.get("user"), "issue user")? {
            return Ok(None);
        }
        let x = self.object_or_empty(item.get("extras"), "extras")?;

        let mut events: Vec<Event> = Vec::new();
        for c in self.list_or_empty(item.get("comments"), "comments")? {
            if !c.is_object() {
                return Err(self.bad("comment"));
            }
            if self.bot(c.get("user"), "comment user")? {
                continue;
            }
            let at = self.key(c, "created_at", "comment created_at")?;
            let login = self.key(
                self.key(c, "user", "comment user")?,
                "login",
                "comment login",
            )?;
            let title = format!(
                "— {} · {}",
                self.shown(login, "comment login")?,
                self.date(at, "comment created_at")?
            );
            events.push((
                self.sort_key(at, "comment created_at")?,
                title,
                self.sort_key(self.key(c, "body", "comment body")?, "comment body")?,
                self.text(
                    self.key(c, "html_url", "comment html_url")?,
                    "comment html_url",
                )?
                .to_string(),
            ));
        }
        for c in self.list_or_empty(item.get("review_comments"), "review_comments")? {
            if !c.is_object() {
                return Err(self.bad("review comment"));
            }
            if self.bot(c.get("user"), "review comment user")? {
                continue;
            }
            // `line`, 없거나 거짓이면 `original_line`, 그것도 아니면 빈 글이에요
            let line = [c.get("line"), c.get("original_line")]
                .into_iter()
                .flatten()
                .find(|v| py::truthy(v));
            let line = match line {
                Some(v) => self.shown(v, "review comment line")?,
                None => String::new(),
            };
            let path = self.shown(
                self.key(c, "path", "review comment path")?,
                "review comment path",
            )?;
            let place = format!("{path}:{line}");
            let login = self.key(
                self.key(c, "user", "review comment user")?,
                "login",
                "review comment login",
            )?;
            let at = self.key(c, "created_at", "review comment created_at")?;
            events.push((
                self.sort_key(at, "review comment created_at")?,
                format!(
                    "— {} · 리뷰 · {}",
                    self.shown(login, "review comment login")?,
                    place.trim_end_matches(':')
                ),
                self.sort_key(
                    self.key(c, "body", "review comment body")?,
                    "review comment body",
                )?,
                self.text(
                    self.key(c, "html_url", "review comment html_url")?,
                    "review comment html_url",
                )?
                .to_string(),
            ));
        }
        let reviews = self.object_or_empty(x.get("reviews"), "extras reviews")?;
        for r in self.list_or_empty(reviews.get("nodes"), "extras reviews nodes")? {
            if !r.is_object() {
                return Err(self.bad("review"));
            }
            let author = self.object_or_empty(r.get("author"), "review author")?;
            let who = match author.get("login") {
                Some(v) if py::truthy(v) => v,
                _ => &Value::Null,
            };
            let body = match r.get("body") {
                Some(v) if py::truthy(v) => self.text(v, "review body")?,
                _ => "",
            };
            if py::strip(body).is_empty() {
                continue;
            }
            let who = if who.is_null() {
                "?"
            } else {
                self.text(who, "review author")?
            };
            if who.ends_with("[bot]") {
                continue;
            }
            let state = self.text(self.key(r, "state", "review state")?, "review state")?;
            events.push((
                self.sort_key(
                    self.key(r, "submittedAt", "review submittedAt")?,
                    "review submittedAt",
                )?,
                format!("— {who} · 리뷰 {}", state.to_lowercase()),
                self.sort_key(self.key(r, "body", "review body")?, "review body")?,
                self.text(self.key(r, "url", "review url")?, "review url")?
                    .to_string(),
            ));
        }

        let n = self.shown(self.key(it, "number", "issue number")?, "issue number")?;
        let is_pr = it.get("pull_request").is_some();
        let kind = if is_pr { "PR" } else { "이슈" };
        let closed = || -> std::io::Result<bool> {
            Ok(self.key(it, "state", "issue state")?.as_str() == Some("closed"))
        };
        let (state, branch) = if is_pr {
            let merged = x.get("mergedAt").is_some_and(py::truthy)
                || self
                    .object_or_empty(
                        Some(self.key(it, "pull_request", "pull_request")?),
                        "pull_request",
                    )?
                    .get("merged_at")
                    .is_some_and(py::truthy);
            let state = if merged {
                "머지됨"
            } else if closed()? {
                "닫힘"
            } else {
                "열림"
            };
            let branch = match x.get("headRefName") {
                Some(head) if py::truthy(head) => format!(
                    " · {} → {}",
                    self.shown(head, "extras headRefName")?,
                    self.shown(
                        x.get("baseRefName")
                            .ok_or_else(|| self.bad("extras baseRefName"))?,
                        "extras baseRefName"
                    )?
                ),
                _ => String::new(),
            };
            (state, branch)
        } else {
            let state = if closed()? { "닫힘" } else { "열림" };
            let labels = match it.get("labels") {
                None => Vec::new(),
                Some(v) => {
                    let mut names = Vec::new();
                    for l in v.as_array().ok_or_else(|| self.bad("labels"))? {
                        names.push(self.text(self.key(l, "name", "label name")?, "label name")?);
                    }
                    names
                }
            };
            let labels = labels.join(", ");
            let branch = if labels.is_empty() {
                String::new()
            } else {
                format!(" · {labels}")
            };
            (state, branch)
        };
        let title = format!(
            "{kind} #{n} · {}",
            self.shown(self.key(it, "title", "issue title")?, "issue title")?
        );
        let login = self.key(self.key(it, "user", "issue user")?, "login", "issue login")?;
        let created = self.date(
            self.key(it, "created_at", "issue created_at")?,
            "issue created_at",
        )?;
        let head = format!(
            "github: {title}\nmeta: {state} · {} · {created}{branch}",
            self.shown(login, "issue login")?
        );
        let html_url = self
            .text(
                self.key(it, "html_url", "issue html_url")?,
                "issue html_url",
            )?
            .to_string();

        // 바뀐 파일이 본문을 열어요(첫 조각만): 테스트는 빼요
        let files_of = self.object_or_empty(x.get("files"), "extras files")?;
        let mut files = Vec::new();
        if let Some(nodes) = files_of.get("nodes") {
            for f in nodes
                .as_array()
                .ok_or_else(|| self.bad("extras files nodes"))?
            {
                let path = self.text(self.key(f, "path", "file path")?, "file path")?;
                if !TESTS.is_match(path) {
                    files.push(path);
                }
            }
        }
        let mut body: Vec<(String, String)> = Vec::new();
        if !files.is_empty() {
            let shown = &files[..files.len().min(SHOWN_FILES)];
            // 정수가 아닌 totalCount는 오류예요
            let total = files_of
                .get("totalCount")
                .and_then(Value::as_i64)
                .ok_or_else(|| self.bad("extras files totalCount"))?;
            let more = total - shown.len() as i64;
            let mut line = format!("files: {}", shown.join(", "));
            if more > 0 {
                line.push_str(&format!(" 외 {more}개"));
            }
            body.push((line, html_url.clone()));
        }
        let description = self.cleaned(it.get("body"), "issue body")?;
        let description = if description.is_empty() {
            "(설명 없음)".to_string()
        } else {
            description
        };
        body.extend(
            description
                .split('\n')
                .map(|l| (l.to_string(), html_url.clone())),
        );
        events.sort();
        for (_, title, text, url) in events {
            let t = clean(text.as_deref().unwrap_or(""));
            if t.is_empty() {
                continue;
            }
            body.push((String::new(), url.clone()));
            body.push((title, url.clone()));
            body.extend(t.split('\n').map(|l| (l.to_string(), url.clone())));
        }

        let author = match login {
            Value::Null => None,
            v => Some(self.text(v, "issue login")?.to_string()),
        };
        Ok(Some(Doc {
            path: format!("github/{}/{n}", if is_pr { "pull" } else { "issue" }),
            title,
            url: html_url,
            author,
            at: created,
            acl: None,
            head,
            body,
        }))
    }
}
