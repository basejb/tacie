//! The order of one sync (docs/31 6절, 10-2절).
//!
//! 1. If paused by the hourly limit (`paused_until` is ahead), it ends without calling Atlassian.
//! 2. Once per connection, `accessible-resources` decides this site's products (Jira, Confluence).
//! 3. An overdue privacy report runs before fetching.
//! 4. Jira changes → initial fetch, Confluence changes → initial fetch (enabled products only). A Jira 404 turns Jira
//!    off.
//! 5. Fetches only the missing Confluence author names (Jira `user/bulk`) and space names.
//! 6. If the initial fetch is finished and the time has come, it reconciles.
//!
//! If the initial fetch or reconcile returns `Later` on the budget, the result is `more`. If it stopped on the limit
//! (`halted`), it is not `more` (it rests until the limit lifts).

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Utc};

use crate::atlassian::api::AtlassianApi;
use crate::atlassian::state::{
    ITEMS, Products, Run, SpaceName, Step, iso, item_ids, json_of, load_state, read_item,
    state_lock,
};
use crate::atlassian::{confluence, jira, privacy, reconcile};
use crate::error::{Kind, SourceError};
use crate::gdrive::api::Got;
use crate::gdrive::fetch::parse;

/// 앱이 넘긴 새 토큰으로도 401인 요청의 실패 문구예요(`Fresh`). 그 제품만 이번 동기화에서 건너뛰어요.
pub(crate) const REFUSED_FRESH: &str = "atlassian: refused with a fresh token";

/// 새 토큰으로도 거절된 실패인지예요.
fn refused_fresh(e: &SourceError) -> bool {
    matches!(e, SourceError::Service { kind: Kind::Down, message } if message == REFUSED_FRESH)
}

/// 새 토큰으로도 거절됐으면 `skipped`를 켜고 `None`이에요. 다른 실패는 그대로예요.
fn or_skip<T>(r: Result<T, SourceError>, skipped: &mut bool) -> Result<Option<T>, SourceError> {
    match r {
        Err(e) if refused_fresh(&e) => {
            *skipped = true;
            Ok(None)
        }
        other => other.map(Some),
    }
}

/// 한 번에 묻는 스페이스 수예요.
const SPACE_BATCH: usize = 250;

/// 동기화 한 번의 결과예요.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fetched {
    /// 예산으로 다 받지 못했어요(다음 동기화가 이어요).
    pub more: bool,
    /// 시간당 한도로 멈췄어요(이번에 멈췄거나 이미 멈춰 있었어요).
    pub halted: bool,
    /// 이번에 `accessible-resources`를 읽었어요.
    pub resources: bool,
    /// 새 토큰으로도 거절된 제품(이나 보고)을 이번에 건너뛰었어요.
    pub skipped: bool,
}

/// 연결 한 번의 값이에요.
pub(crate) struct Site<'a> {
    pub cloud: &'a str,
    pub site: Option<&'a str>,
    pub site_url: Option<&'a str>,
    /// 이 연결에서 아직 `accessible-resources`를 읽지 않았어요. 저장본에 제품이 하나도 없으면 이 값과 상관없이 읽어요.
    pub load_products: bool,
}

/// 지금 멈춰 있는지예요(`paused_until`이 `now`보다 뒤예요).
pub(crate) fn paused_at(raw: &Path, now: DateTime<Utc>) -> Option<String> {
    let s = {
        let _held = state_lock();
        load_state(raw)
    };
    s.paused_until.filter(|t| parse(t).is_some_and(|t| t > now))
}

/// Jira가 이 사이트에 없다는 404인지예요(검색이나 `myself`가 404).
fn jira_missing(e: &SourceError) -> bool {
    matches!(e, SourceError::Service { kind: Kind::Down, message }
        if message == "atlassian: jira search failed 404"
            || message == "atlassian: jira myself failed 404")
}

/// Jira 한 걸음이에요. 404면 Jira를 끄고 저장한 뒤 `Done`이에요.
fn jira_step(
    run: &mut Run,
    f: fn(&mut Run) -> Result<Step, SourceError>,
) -> Result<Step, SourceError> {
    match f(run) {
        Err(e) if jira_missing(&e) => {
            let mut p = run.state.products;
            p.jira = false;
            run.state.products = p;
            run.save_products(p)?;
            Ok(Step::Done)
        }
        other => other,
    }
}

/// With an API token there is no `accessible-resources`: ask each product once. A 2xx means the site has it, another
/// answer means it does not; a refused token stays an error.
pub(crate) fn probe_products(api: &dyn AtlassianApi, cloud: &str) -> Result<Products, SourceError> {
    let has = |path: String, query: &[(&str, String)]| -> Result<bool, SourceError> {
        Ok(matches!(api.get(&path, query)?, Got::Body(_)))
    };
    Ok(Products {
        jira: has(format!("ex/jira/{cloud}/rest/api/3/myself"), &[])?,
        confluence: has(
            format!("ex/confluence/{cloud}/wiki/api/v2/spaces"),
            &[("limit", "1".to_string())],
        )?,
    })
}

/// `raw`를 새로 받아요. 차례는 모듈 설명이에요.
pub(crate) fn fetch_with(
    api: &dyn AtlassianApi,
    raw: &Path,
    now: DateTime<Utc>,
    site: &Site,
    still: &dyn Fn() -> bool,
    jitter: &dyn Fn() -> i64,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Fetched, SourceError> {
    if paused_at(raw, now).is_some() {
        return Ok(Fetched {
            more: false,
            halted: true,
            resources: false,
            skipped: false,
        });
    }
    let state = {
        let _held = state_lock();
        load_state(raw)
    };
    let mut run = Run {
        api,
        dir: raw,
        cloud: site.cloud,
        now,
        still,
        state,
        progress,
        jitter,
        halted: None,
    };
    run.state.site = site.site.map(str::to_string).or(run.state.site.take());
    run.state.site_url = site
        .site_url
        .map(str::to_string)
        .or(run.state.site_url.take());
    let mut resources = false;
    // 저장본이 없으면(7일 뒤 지웠어요) 연결 세대와 상관없이 제품을 다시 읽어요. 받은 적이 있는데 제품이 없는
    // 사이트는 동기화마다 다시 묻지 않아요.
    let unknown = run.state.fetched_at.is_none()
        && !run.state.products.jira
        && !run.state.products.confluence;
    if site.load_products || unknown {
        run.guard()?;
        let found = if api.is_basic() {
            probe_products(api, site.cloud)
        } else {
            api.get("oauth/token/accessible-resources", &[])
                .and_then(|g| json_of(g, "accessible resources"))
                .map(|v| confluence::products(&v, site.cloud))
        };
        match found {
            Ok(p) => {
                run.state.products = p;
                run.save_products(p)?;
                resources = true;
            }
            Err(e) => {
                run.pause_on_quota(e)?;
            }
        }
    }
    // 새 토큰으로도 거절된 쪽은 이번 동기화에서 건너뛰어요(다음 동기화가 다시 해요)
    let mut skipped = false;
    // the personal data report is an OAuth app's duty; an API token (the open edition) has no app to report for
    if run.halted.is_none() && !api.is_basic() && privacy::report_due(&run.state, now) {
        or_skip(privacy::report(&mut run), &mut skipped)?;
    }
    let mut later = false;
    // 받으려 한 제품 수와 그중 새 토큰으로도 거절된 수예요
    let (mut tried, mut refused) = (0, 0);
    if run.state.products.jira {
        tried += 1;
        let mut no = false;
        if or_skip(jira_step(&mut run, jira::changes), &mut no)?.is_some()
            && run.state.products.jira
            && let Some(step) = or_skip(jira_step(&mut run, jira::initial), &mut no)?
        {
            later |= step == Step::Later;
        }
        refused += usize::from(no);
        skipped |= no;
    }
    if run.state.products.confluence {
        tried += 1;
        let mut no = false;
        if or_skip(confluence::changes(&mut run), &mut no)?.is_some()
            && let Some(step) = or_skip(confluence::initial(&mut run), &mut no)?
        {
            later |= step == Step::Later;
        }
        refused += usize::from(no);
        skipped |= no;
    }
    // 모든 제품이 새 토큰으로도 거절됐어요: 토큰은 살아 있어도 사이트가 연결을 거둔 거예요(관리자가 앱을 뺐거나
    // 사이트에서 빠졌어요). 끊김(`key`)으로 돌려서 `gather`가 `lost_since`를 적게 해요.
    if tried > 0 && refused == tried {
        return Err(SourceError::service(
            Kind::Key,
            "atlassian: every product refused a fresh token",
        ));
    }
    if run.halted.is_none()
        && let Err(e) = names(&mut run).and_then(|()| spaces(&mut run))
    {
        if refused_fresh(&e) {
            skipped = true;
        } else {
            run.pause_on_quota(e)?;
        }
    }
    if !later && run.halted.is_none() && !skipped {
        later |= reconcile::reconcile(&mut run)? == Step::Later;
    }
    run.state.fetched_at = Some(iso(now));
    run.save()?;
    std::fs::create_dir_all(raw.join(ITEMS))?;
    let halted = run.halted.is_some();
    Ok(Fetched {
        more: later && !halted,
        halted,
        resources,
        skipped,
    })
}

/// Confluence 글 파일(글이 있는 것)이에요.
fn pages(run: &Run) -> Vec<serde_json::Value> {
    item_ids(run.dir, "page")
        .into_iter()
        .filter_map(|id| read_item(run.dir, "page", &id))
        .filter(|v| v["page"].is_object() || v["comments"].is_array())
        .collect()
}

/// Confluence 글쓴이·댓글 작성자 이름을 받아 둬요. Jira가 있는 사이트만이에요(`user/bulk`는 Jira API예요). 원본에
/// 없는 id는 지워요.
fn names(run: &mut Run) -> Result<(), SourceError> {
    let mut ids = BTreeSet::new();
    for v in pages(run) {
        let people = std::iter::once(&v["page"]["authorId"]).chain(
            v["comments"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|c| [&c["version"]["authorId"], &c["authorId"]]),
        );
        ids.extend(
            people
                .filter_map(|x| x.as_str())
                .filter(|x| !x.is_empty())
                .map(str::to_string),
        );
    }
    run.state.names.retain(|id, _| ids.contains(id));
    if !run.state.products.jira {
        return Ok(());
    }
    let missing: BTreeSet<String> = ids
        .into_iter()
        .filter(|id| !run.state.names.contains_key(id) && !run.state.closed.contains(id))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let got = privacy::user_names(run, &missing)?;
    run.state.names.extend(got);
    Ok(())
}

/// Confluence 스페이스 이름을 받아 둬요. 원본에 없는 스페이스는 지워요.
fn spaces(run: &mut Run) -> Result<(), SourceError> {
    if !run.state.products.confluence {
        return Ok(());
    }
    let ids: BTreeSet<String> = pages(run)
        .iter()
        .filter_map(|v| confluence::sid(&v["page"]["spaceId"]))
        .collect();
    run.state.spaces.retain(|id, _| ids.contains(id));
    let missing: Vec<String> = ids
        .into_iter()
        .filter(|id| !run.state.spaces.contains_key(id))
        .collect();
    let path = format!("ex/confluence/{}/wiki/api/v2/spaces", run.cloud);
    for chunk in missing.chunks(SPACE_BATCH) {
        run.guard()?;
        let q = [("ids", chunk.join(",")), ("limit", SPACE_BATCH.to_string())];
        let got = run.api.get(&path, &q)?;
        let v = match got {
            // 스페이스 이름은 없어도 찾을 수 있어요. 다음 동기화가 다시 물어요.
            Got::Fail { .. } => {
                run.state.points.charge(run.now, 1, 0, 0);
                return Ok(());
            }
            body => json_of(body, "confluence spaces")?,
        };
        let results = v["results"].as_array().cloned().unwrap_or_default();
        run.state.points.charge(
            run.now,
            1,
            u32::try_from(results.len()).unwrap_or(u32::MAX),
            0,
        );
        for sp in results {
            if let Some(id) = confluence::sid(&sp["id"]) {
                run.state.spaces.insert(
                    id,
                    SpaceName {
                        key: sp["key"].as_str().unwrap_or("").to_string(),
                        name: sp["name"].as_str().unwrap_or("").to_string(),
                    },
                );
            }
        }
    }
    Ok(())
}

/// 처음 받기를 끝냈는지예요(켜진 제품마다).
pub(crate) fn initial_done(raw: &Path) -> bool {
    let s = load_state(raw);
    let p = s.products;
    (!p.jira || s.jira.done)
        && (!p.confluence
            || [&s.pages, &s.blogs, &s.footer, &s.inline]
                .iter()
                .all(|c| c.done))
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    use crate::atlassian::api::Answer;
    use serde_json::Value;

    struct Site(bool, bool);

    impl AtlassianApi for Site {
        fn get(&self, path: &str, _q: &[(&str, String)]) -> Result<Got, SourceError> {
            let on = if path.contains("/rest/api/3/myself") {
                self.0
            } else {
                self.1
            };
            Ok(if on {
                Got::Body(b"{}".to_vec())
            } else {
                Got::Fail {
                    status: 404,
                    reason: String::new(),
                }
            })
        }
        fn post(&self, _p: &str, _b: &Value) -> Result<Answer, SourceError> {
            unreachable!()
        }
        fn is_basic(&self) -> bool {
            true
        }
    }

    #[test]
    fn probes_find_each_product() {
        let p = probe_products(&Site(true, false), "c").unwrap();
        assert!(p.jira && !p.confluence);
        let p = probe_products(&Site(false, true), "c").unwrap();
        assert!(!p.jira && p.confluence);
    }
}
