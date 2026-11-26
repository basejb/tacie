//! A fake Atlassian (`AtlassianApi`): holds Jira issues, comments, Confluence items and users in memory. Fetch,
//! reconcile and report tests share it. It mimics only as much as needed; widen it when the next piece of work needs
//! more.
//!
//! - Every call is recorded in `log` as `"GET <path>"` / `"POST <path>"` (query dropped). Used to check order.
//! - Order: `refuse` (401) → `quota` (the hourly limit, once only) → routes. If `confluence` is false, `/wiki/` routes
//!   are 404.
//! - `search/jql`: issues live in `jira`, and `fields.updated` is compared and sorted **down to milliseconds** like
//!   real Jira (ties at the same instant are split by key). A JQL time string `"yyyy/MM/dd HH:mm"` is second 0 of
//!   that minute (read as UTC, time zone ignored).
//!   In JQL it reads only `updated <= "…"`, `<`, `>=`, `>`, `updated >= -730d`, `project not in (A, B)`,
//!   `ORDER BY updated DESC, key DESC` / `ORDER BY updated ASC`. Paging is `maxResults` (default 100) and
//!   `nextPageToken` (the next start index).
//! - `issue/bulkfetch` matches by id or key, and `issue/<key>/comment` returns only entries in `comments` whose
//!   `issue` field (key or id) matches (a field real comments do not have; only the fake uses it).
//! - Confluence v2 (`pages`, `blogposts`, `footer-comments`, `inline-comments`): `status` (comma-separated, default
//!   `current`), `id` (comma-separated), `sort` (`modified-date`, `-modified-date`; keyed on `version.createdAt`),
//!   `limit` (default 25), `cursor`.
//!   The next page is the `cursor=` in `_links.next`. A non-numeric cursor is 400. Comment routes under an item
//!   (`pages/<id>/footer-comments`, `blogposts/<id>/inline-comments` and so on) return, from the same list, only
//!   top-level comments (no `parentCommentId`) whose `pageId`/`blogPostId` matches. Replies come from
//!   `footer-comments/<id>/children`/`inline-comments/<id>/children` by `parentCommentId`. The site-wide lists return
//!   replies too.
//! - `app/report-accounts/`: if any sent account is in `closed`/`updated`, 200 and
//!   `{"accounts":[{accountId,status:"closed"|"updated"}]}`, otherwise 204. `Cycle-Period` is always 604800. Sent
//!   bodies are kept in `reports`.
//! - If `reject_tokens` is true, only `oauth/token/accessible-resources` is 401.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, NaiveDateTime, Timelike, Utc};
use serde_json::{Value, json};

use crate::atlassian::api::{Answer, AtlassianApi};
use crate::error::{Kind, SourceError};
use crate::gdrive::api::Got;

pub struct FakeAtlassian {
    /// Jira 글: `{id, key, fields: {updated, project?: {key}, …}}`.
    pub jira: Mutex<Vec<Value>>,
    /// Jira 댓글: `{issue: "<key 또는 id>", id, …}`.
    pub comments: Mutex<Vec<Value>>,
    pub pages: Mutex<Vec<Value>>,
    pub blogs: Mutex<Vec<Value>>,
    pub footer: Mutex<Vec<Value>>,
    pub inline: Mutex<Vec<Value>>,
    /// 사용자: `{accountId, displayName, …}`.
    pub users: Mutex<Vec<Value>>,
    /// Confluence 스페이스: `{id, key, name}`. `spaces?ids=…`(쉼표)가 id로 골라 줘요.
    pub spaces: Mutex<Vec<Value>>,
    /// `accessible-resources`의 답이에요(처음엔 사이트 하나).
    pub resources: Mutex<Vec<Value>>,
    /// 부른 길이에요: `"GET <path>"` / `"POST <path>"`.
    pub log: Mutex<Vec<String>>,
    /// `accessible-resources`가 토큰을 거절해요(401).
    pub reject_tokens: Mutex<bool>,
    /// 모든 호출이 401이에요.
    pub refuse: AtomicBool,
    /// 다음 호출 하나가 시간당 한도예요. 값은 풀리는 때(RFC 3339)이고 쓰면 비워져요.
    pub quota: Mutex<Option<String>>,
    /// false면 Confluence 길이 404예요.
    pub confluence: AtomicBool,
    /// 탈퇴로 보고할 계정 id예요.
    pub closed: Mutex<Vec<String>>,
    /// 바뀐 것으로 보고할 계정 id예요(이름은 `users`에서 다시 받아요).
    pub updated: Mutex<Vec<String>>,
    /// `app/report-accounts/`로 온 몸통이에요.
    pub reports: Mutex<Vec<Value>>,
}

impl Default for FakeAtlassian {
    fn default() -> Self {
        Self::new()
    }
}

fn q(query: &[(&str, String)], k: &str) -> Option<String> {
    query.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())
}

fn body(v: Value) -> Result<Got, SourceError> {
    Ok(Got::Body(v.to_string().into_bytes()))
}

fn missing() -> Result<Got, SourceError> {
    Ok(Got::Fail {
        status: 404,
        reason: String::new(),
    })
}

/// 분 단위 UTC예요. RFC 3339나 `yyyy/MM/dd HH:mm`을 읽어요.
fn minute(s: &str) -> Option<NaiveDateTime> {
    let t = DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc).naive_utc())
        .or_else(|_| {
            DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z")
                .map(|d| d.with_timezone(&Utc).naive_utc())
        })
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y/%m/%d %H:%M"))
        .ok()?;
    t.with_second(0)?.with_nanosecond(0)
}

/// 밀리초까지의 UTC 시각이에요(이슈의 `updated`).
fn instant(s: &str) -> Option<NaiveDateTime> {
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .map(|d| d.with_timezone(&Utc).naive_utc())
        .ok()
}

fn updated_of(issue: &Value) -> Option<NaiveDateTime> {
    instant(issue["fields"]["updated"].as_str()?)
}

/// `KEY-12` → (`KEY`, 12)예요(정렬용).
fn key_parts(issue: &Value) -> (String, u64) {
    let k = issue["key"].as_str().unwrap_or("");
    match k.rsplit_once('-') {
        Some((p, n)) => (p.to_string(), n.parse().unwrap_or(0)),
        None => (k.to_string(), 0),
    }
}

fn project_of(issue: &Value) -> String {
    issue["fields"]["project"]["key"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| key_parts(issue).0)
}

/// JQL에서 `updated <op> "…"` 또는 `updated >= -Nd`의 (연산자, 시각)을 모두 읽어요.
fn updated_bounds(jql: &str) -> Vec<(String, NaiveDateTime)> {
    let mut out = Vec::new();
    let mut rest = jql;
    while let Some(i) = rest.find("updated ") {
        rest = &rest[i + "updated ".len()..];
        let op: String = rest.chars().take_while(|c| "<>=".contains(*c)).collect();
        if op.is_empty() {
            continue;
        }
        let val = rest[op.len()..].trim_start();
        if let Some(v) = val.strip_prefix('"') {
            if let Some(end) = v.find('"')
                && let Some(t) = minute(&v[..end])
            {
                out.push((op, t));
            }
        } else if let Some(d) = val.strip_prefix('-') {
            let n: String = d.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(days) = n.parse::<i64>() {
                let t = (Utc::now() - chrono::Duration::days(days)).naive_utc();
                out.push((op, t.with_second(0).unwrap_or(t)));
            }
        }
    }
    out
}

/// `project not in (A, B)`의 프로젝트들이에요.
fn excluded_projects(jql: &str) -> Vec<String> {
    let Some(i) = jql.find("project not in") else {
        return Vec::new();
    };
    let rest = &jql[i..];
    let (Some(a), Some(b)) = (rest.find('('), rest.find(')')) else {
        return Vec::new();
    };
    rest[a + 1..b]
        .split(',')
        .map(|p| p.trim().trim_matches('"').to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// 쪽 나누기: `start`부터 `size`개와 다음 시작 번호(남았으면)예요.
fn page(items: Vec<Value>, start: usize, size: usize) -> (Vec<Value>, Option<usize>) {
    let end = (start + size).min(items.len());
    let next = (end < items.len()).then_some(end);
    (items.get(start..end).unwrap_or(&[]).to_vec(), next)
}

impl FakeAtlassian {
    pub fn new() -> FakeAtlassian {
        FakeAtlassian {
            jira: Mutex::default(),
            comments: Mutex::default(),
            pages: Mutex::default(),
            blogs: Mutex::default(),
            footer: Mutex::default(),
            inline: Mutex::default(),
            users: Mutex::default(),
            spaces: Mutex::default(),
            resources: Mutex::new(vec![json!({
                "id": "cloud-1", "url": "https://acme.atlassian.net", "name": "acme",
                "scopes": ["read:jira-work"]
            })]),
            log: Mutex::default(),
            reject_tokens: Mutex::default(),
            refuse: AtomicBool::new(false),
            quota: Mutex::default(),
            confluence: AtomicBool::new(true),
            closed: Mutex::default(),
            updated: Mutex::default(),
            reports: Mutex::default(),
        }
    }

    /// 부른 길이에요.
    pub fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn gate(&self, line: String) -> Result<(), SourceError> {
        self.log.lock().unwrap().push(line);
        if self.refuse.load(Ordering::SeqCst) {
            return Err(SourceError::service(Kind::Key, "atlassian: refused"));
        }
        if let Some(t) = self.quota.lock().unwrap().take() {
            return Err(SourceError::service(
                Kind::Busy,
                format!("atlassian: quota until {t}"),
            ));
        }
        Ok(())
    }

    fn search(&self, query: &[(&str, String)]) -> Result<Got, SourceError> {
        let jql = q(query, "jql").unwrap_or_default();
        let bounds = updated_bounds(&jql);
        let skip = excluded_projects(&jql);
        let mut issues: Vec<Value> = self
            .jira
            .lock()
            .unwrap()
            .iter()
            .filter(|i| {
                let Some(u) = updated_of(i) else { return false };
                bounds.iter().all(|(op, t)| match op.as_str() {
                    "<=" => u <= *t,
                    "<" => u < *t,
                    ">=" => u >= *t,
                    ">" => u > *t,
                    _ => true,
                }) && !skip.contains(&project_of(i))
            })
            .cloned()
            .collect();
        let descending = jql.to_lowercase().contains("order by updated desc");
        issues.sort_by_key(|i| (updated_of(i), key_parts(i)));
        if descending {
            issues.reverse();
        }
        let size = q(query, "maxResults")
            .and_then(|s| s.parse().ok())
            .unwrap_or(100);
        let start = q(query, "nextPageToken")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let (items, next) = page(issues, start, size);
        let mut out = json!({"issues": items});
        if let Some(n) = next {
            out["nextPageToken"] = json!(n.to_string());
        }
        body(out)
    }

    fn bulk_fetch(&self, req: &Value) -> Value {
        let wanted: Vec<String> = req["issueIdsOrKeys"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let issues: Vec<Value> = self
            .jira
            .lock()
            .unwrap()
            .iter()
            .filter(|i| {
                wanted.iter().any(|w| {
                    i["id"].as_str() == Some(w.as_str()) || i["key"].as_str() == Some(w.as_str())
                })
            })
            .cloned()
            .collect();
        json!({"issues": issues, "issueErrors": []})
    }

    fn issue_comments(&self, issue: &str, query: &[(&str, String)]) -> Result<Got, SourceError> {
        let all: Vec<Value> = self
            .comments
            .lock()
            .unwrap()
            .iter()
            .filter(|c| {
                c["issue"].as_str() == Some(issue)
                    || c["issue"].as_u64().map(|n| n.to_string()).as_deref() == Some(issue)
            })
            .cloned()
            .collect();
        let total = all.len();
        let size = q(query, "maxResults")
            .and_then(|s| s.parse().ok())
            .unwrap_or(100);
        let start = q(query, "startAt")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let (items, _) = page(all, start, size);
        body(json!({"comments": items, "startAt": start, "maxResults": size, "total": total}))
    }

    fn user_bulk(&self, query: &[(&str, String)]) -> Result<Got, SourceError> {
        let ids: Vec<&String> = query
            .iter()
            .filter(|(k, _)| *k == "accountId")
            .map(|(_, v)| v)
            .collect();
        let values: Vec<Value> = self
            .users
            .lock()
            .unwrap()
            .iter()
            .filter(|u| {
                ids.iter()
                    .any(|i| u["accountId"].as_str() == Some(i.as_str()))
            })
            .cloned()
            .collect();
        body(json!({"values": values, "isLast": true}))
    }

    /// Confluence v2 목록 하나예요.
    /// `keep`이 참인 것만이에요(글에 딸린 맨 위 댓글, 답글).
    fn wiki_list(
        &self,
        source: &Mutex<Vec<Value>>,
        path: &str,
        query: &[(&str, String)],
        keep: &dyn Fn(&Value) -> bool,
    ) -> Result<Got, SourceError> {
        let statuses: Vec<String> = q(query, "status")
            .unwrap_or_else(|| "current".into())
            .split(',')
            .map(str::to_string)
            .collect();
        let ids: Option<Vec<String>> =
            q(query, "id").map(|s| s.split(',').map(str::to_string).collect());
        let mut items: Vec<Value> = source
            .lock()
            .unwrap()
            .iter()
            .filter(|p| {
                statuses
                    .iter()
                    .any(|s| p["status"].as_str().unwrap_or("current") == s)
                    && ids
                        .as_ref()
                        .is_none_or(|ids| ids.iter().any(|i| p["id"].as_str() == Some(i.as_str())))
                    && keep(p)
            })
            .cloned()
            .collect();
        let stamp = |p: &Value| p["version"]["createdAt"].as_str().and_then(minute_full);
        items.sort_by_key(|p| (stamp(p), p["id"].as_str().map(str::to_string)));
        if q(query, "sort").is_some_and(|s| s.starts_with('-')) {
            items.reverse();
        }
        let size = q(query, "limit").and_then(|s| s.parse().ok()).unwrap_or(25);
        // 읽을 수 없는 커서는 진짜처럼 400이에요.
        let start = match q(query, "cursor") {
            None => 0,
            Some(c) => match c.parse() {
                Ok(n) => n,
                Err(_) => {
                    return Ok(Got::Fail {
                        status: 400,
                        reason: String::new(),
                    });
                }
            },
        };
        let (results, next) = page(items, start, size);
        let mut out = json!({"results": results, "_links": {}});
        if let Some(n) = next {
            out["_links"]["next"] = json!(format!("/{path}?limit={size}&cursor={n}"));
        }
        body(out)
    }
}

/// 초까지 견주는 시각이에요(Confluence 정렬용).
fn minute_full(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

impl AtlassianApi for FakeAtlassian {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Got, SourceError> {
        self.gate(format!("GET {path}"))?;
        if path.contains("/wiki/api/v2/") {
            if !self.confluence.load(Ordering::SeqCst) {
                return missing();
            }
            let rest = path.split("/wiki/api/v2/").nth(1).unwrap_or("");
            let parts: Vec<&str> = rest.split('/').collect();
            let comments = |list: &str| match list {
                "footer-comments" => Some(&self.footer),
                "inline-comments" => Some(&self.inline),
                _ => None,
            };
            if let [list, id, "children"] = parts.as_slice() {
                let Some(src) = comments(list) else {
                    return missing();
                };
                let keep = |c: &Value| c["parentCommentId"].as_str() == Some(*id);
                return self.wiki_list(src, path, query, &keep);
            }
            if let [kind, id, list] = parts.as_slice() {
                let field = match *kind {
                    "pages" => "pageId",
                    "blogposts" => "blogPostId",
                    _ => return missing(),
                };
                let Some(src) = comments(list) else {
                    return missing();
                };
                let keep = |c: &Value| {
                    c[field].as_str() == Some(*id) && c.get("parentCommentId").is_none()
                };
                return self.wiki_list(src, path, query, &keep);
            }
            let all = |_: &Value| true;
            let list = |src: &Mutex<Vec<Value>>| self.wiki_list(src, path, query, &all);
            return match path.rsplit('/').next().unwrap_or("") {
                "pages" => list(&self.pages),
                "blogposts" => list(&self.blogs),
                "footer-comments" => list(&self.footer),
                "inline-comments" => list(&self.inline),
                "spaces" => {
                    let ids: Vec<String> = q(query, "ids")
                        .map(|s| s.split(',').map(str::to_string).collect())
                        .unwrap_or_default();
                    let results: Vec<Value> = self
                        .spaces
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|sp| ids.iter().any(|i| sp["id"].as_str() == Some(i.as_str())))
                        .cloned()
                        .collect();
                    body(json!({"results": results, "_links": {}}))
                }
                _ => missing(),
            };
        }
        if path.ends_with("rest/api/3/search/jql") {
            return self.search(query);
        }
        if path.ends_with("rest/api/3/myself") {
            return body(json!({"accountId": "me", "displayName": "Me"}));
        }
        if path.ends_with("rest/api/3/user/bulk") {
            return self.user_bulk(query);
        }
        if let Some((_, rest)) = path.split_once("rest/api/3/issue/")
            && let Some(key) = rest.strip_suffix("/comment")
        {
            return self.issue_comments(key, query);
        }
        if path.ends_with("oauth/token/accessible-resources") {
            if *self.reject_tokens.lock().unwrap() {
                return Err(SourceError::service(Kind::Key, "atlassian: refused"));
            }
            return body(Value::Array(self.resources.lock().unwrap().clone()));
        }
        missing()
    }

    fn post(&self, path: &str, request: &Value) -> Result<Answer, SourceError> {
        self.gate(format!("POST {path}"))?;
        let json_answer = |v: Value| Answer {
            status: 200,
            body: v.to_string().into_bytes(),
            cycle_period: Some(604_800),
        };
        if path.ends_with("rest/api/3/issue/bulkfetch") {
            return Ok(json_answer(self.bulk_fetch(request)));
        }
        if path.ends_with("app/report-accounts/") {
            self.reports.lock().unwrap().push(request.clone());
            let closed = self.closed.lock().unwrap().clone();
            let updated = self.updated.lock().unwrap().clone();
            let hit: Vec<Value> = request["accounts"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x["accountId"].as_str())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
                .into_iter()
                .filter_map(|id| {
                    if closed.iter().any(|c| c == id) {
                        Some(json!({"accountId": id, "status": "closed"}))
                    } else if updated.iter().any(|c| c == id) {
                        Some(json!({"accountId": id, "status": "updated"}))
                    } else {
                        None
                    }
                })
                .collect();
            if hit.is_empty() {
                return Ok(Answer {
                    status: 204,
                    body: Vec::new(),
                    cycle_period: Some(604_800),
                });
            }
            return Ok(json_answer(json!({"accounts": hit})));
        }
        Ok(Answer {
            status: 404,
            body: Vec::new(),
            cycle_period: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlassian::api::quota_until;

    fn issue(id: &str, key: &str, updated: &str) -> Value {
        json!({"id": id, "key": key, "fields": {"updated": updated}})
    }

    fn keys(got: Got) -> Vec<String> {
        got.json("search").unwrap()["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_string())
            .collect()
    }

    fn seeded() -> FakeAtlassian {
        let f = FakeAtlassian::new();
        *f.jira.lock().unwrap() = vec![
            issue("1", "A-1", "2026-11-10T01:00:00.000+0900"),
            issue("2", "A-2", "2026-11-11T10:30:00.000+0000"),
            issue("3", "B-1", "2026-11-11T10:30:00.000+0000"),
            issue("4", "A-3", "2026-11-12T00:00:00.000+0000"),
        ];
        f
    }

    #[test]
    fn jql_bounds_order_and_paging() {
        let f = seeded();
        let jql = |s: &str| vec![("jql", s.to_string())];
        // A-1은 UTC로 2026-11-09 16:00이에요.
        let got = f
            .get(
                "rest/api/3/search/jql",
                &jql("updated <= \"2026/11/11 10:30\" ORDER BY updated DESC, key DESC"),
            )
            .unwrap();
        assert_eq!(keys(got), ["B-1", "A-2", "A-1"]);
        let got = f
            .get(
                "rest/api/3/search/jql",
                &jql("updated > \"2026/11/11 10:30\" ORDER BY updated ASC"),
            )
            .unwrap();
        assert_eq!(keys(got), ["A-3"]);
        let got = f
            .get(
                "rest/api/3/search/jql",
                &jql("project not in (A) ORDER BY updated ASC"),
            )
            .unwrap();
        assert_eq!(keys(got), ["B-1"]);
        let mut q = jql("ORDER BY updated ASC");
        q.push(("maxResults", "3".into()));
        let first = f
            .get("rest/api/3/search/jql", &q)
            .unwrap()
            .json("s")
            .unwrap();
        assert_eq!(first["issues"].as_array().unwrap().len(), 3);
        q.push((
            "nextPageToken",
            first["nextPageToken"].as_str().unwrap().into(),
        ));
        let second = f
            .get("rest/api/3/search/jql", &q)
            .unwrap()
            .json("s")
            .unwrap();
        assert_eq!(second["issues"].as_array().unwrap().len(), 1);
        assert!(second.get("nextPageToken").is_none());
    }

    #[test]
    fn the_730_day_window_drops_old_issues() {
        let f = FakeAtlassian::new();
        let now = Utc::now().to_rfc3339();
        *f.jira.lock().unwrap() = vec![
            issue("1", "A-1", "2020-01-01T00:00:00.000+0000"),
            issue("2", "A-2", &now),
        ];
        let got = f
            .get(
                "rest/api/3/search/jql",
                &[("jql", "updated >= -730d ORDER BY updated ASC".into())],
            )
            .unwrap();
        assert_eq!(keys(got), ["A-2"]);
    }

    #[test]
    fn quota_answers_once_and_refuse_is_key() {
        let f = FakeAtlassian::new();
        *f.quota.lock().unwrap() = Some("2026-11-12T06:00:00Z".into());
        let e = f.get("rest/api/3/myself", &[]).unwrap_err();
        assert_eq!(e.kind(), Kind::Busy);
        assert!(quota_until(&e).is_some());
        assert!(f.get("rest/api/3/myself", &[]).is_ok());
        f.refuse.store(true, Ordering::SeqCst);
        assert_eq!(
            f.get("rest/api/3/myself", &[]).unwrap_err().kind(),
            Kind::Key
        );
        assert_eq!(f.calls().len(), 3);
        assert_eq!(f.calls()[0], "GET rest/api/3/myself");
    }

    #[test]
    fn confluence_lists_filter_sort_and_404_when_off() {
        let f = FakeAtlassian::new();
        let page = |id: &str, status: &str, at: &str| json!({"id": id, "status": status, "version": {"createdAt": at}});
        *f.pages.lock().unwrap() = vec![
            page("1", "current", "2026-11-01T00:00:00Z"),
            page("2", "archived", "2026-11-02T00:00:00Z"),
            page("3", "current", "2026-11-03T00:00:00Z"),
        ];
        let p = "ex/confluence/c/wiki/api/v2/pages";
        let ids = |g: Got| -> Vec<String> {
            g.json("p").unwrap()["results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            ids(f.get(p, &[("sort", "-modified-date".into())]).unwrap()),
            ["3", "1"]
        );
        assert_eq!(
            ids(f
                .get(
                    p,
                    &[
                        ("status", "current,archived".into()),
                        ("sort", "modified-date".into()),
                        ("limit", "2".into())
                    ]
                )
                .unwrap()),
            ["1", "2"]
        );
        let next = f
            .get(
                p,
                &[("status", "current,archived".into()), ("limit", "2".into())],
            )
            .unwrap()
            .json("p")
            .unwrap();
        assert!(
            next["_links"]["next"]
                .as_str()
                .unwrap()
                .contains("cursor=2")
        );
        assert_eq!(
            ids(f
                .get(p, &[("id", "2".into()), ("status", "archived".into())])
                .unwrap()),
            ["2"]
        );
        f.confluence.store(false, Ordering::SeqCst);
        assert_eq!(
            f.get(p, &[]).unwrap(),
            Got::Fail {
                status: 404,
                reason: String::new()
            }
        );
    }

    #[test]
    fn report_accounts_answers_204_or_closed() {
        let f = FakeAtlassian::new();
        let req = json!({"accounts": [{"accountId": "u1", "updatedAt": "2026-11-12T00:00:00Z"}]});
        let a = f.post("app/report-accounts/", &req).unwrap();
        assert_eq!((a.status, a.cycle_period), (204, Some(604_800)));
        f.closed.lock().unwrap().push("u1".into());
        let a = f.post("app/report-accounts/", &req).unwrap();
        assert_eq!(a.status, 200);
        let v: Value = serde_json::from_slice(&a.body).unwrap();
        assert_eq!(v["accounts"][0]["status"], "closed");
        assert_eq!(f.reports.lock().unwrap().len(), 2);
    }

    #[test]
    fn bulkfetch_comments_users_and_resources() {
        let f = seeded();
        f.comments
            .lock()
            .unwrap()
            .push(json!({"issue": "A-1", "id": "c1"}));
        f.users
            .lock()
            .unwrap()
            .push(json!({"accountId": "u1", "displayName": "Kim"}));
        let a = f
            .post(
                "ex/jira/c/rest/api/3/issue/bulkfetch",
                &json!({"issueIdsOrKeys": ["A-1", "4"]}),
            )
            .unwrap();
        let v: Value = serde_json::from_slice(&a.body).unwrap();
        assert_eq!(v["issues"].as_array().unwrap().len(), 2);
        let c = f
            .get("ex/jira/c/rest/api/3/issue/A-1/comment", &[])
            .unwrap()
            .json("c")
            .unwrap();
        assert_eq!(c["total"], 1);
        let u = f
            .get(
                "ex/jira/c/rest/api/3/user/bulk",
                &[("accountId", "u1".into()), ("accountId", "u9".into())],
            )
            .unwrap()
            .json("u")
            .unwrap();
        assert_eq!(u["values"].as_array().unwrap().len(), 1);
        let r = f
            .get("oauth/token/accessible-resources", &[])
            .unwrap()
            .json("r")
            .unwrap();
        assert_eq!(r[0]["id"], "cloud-1");
        *f.reject_tokens.lock().unwrap() = true;
        assert!(f.get("oauth/token/accessible-resources", &[]).is_err());
    }
}
