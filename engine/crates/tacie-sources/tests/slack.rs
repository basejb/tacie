//! The Slack source. The golden files (slack_chunks.json, slack_unescape.json) were made from the same raw data. They
//! are fixed values made by the old Python engine (2026-10-26).
//!
//! Slack calls are answered by `SlackApi` fakes (`FakeApi`, `GoldenApi`): the seam `fetch_with` takes. `SlackHttp` is
//! checked against one local HTTP fake (real Slack is never called).

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use tacie_index::chunk::chunk_of;
use tacie_index::mask::{Mask, allowed};
use tacie_index::{RawChunk, pyjson};
use tacie_sources::error::{Kind, SourceError};
use tacie_sources::py;
use tacie_sources::slack::{
    self, API, DAYS, SlackApi, SlackFetch, SlackHttp, SlackSource, chunks, clean, documents,
    fetch_with, token,
};
use tacie_sources::testing::FakeSlack;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(Path::new(FIXTURES).join(name)).unwrap()).unwrap()
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn write(path: &Path, v: &Value) {
    fs::write(path, pyjson::to_string(v, None)).unwrap();
}

/// 값도 키 순서도 golden과 같아요.
fn assert_same_json(got: &Value, want: &Value, what: &str) {
    assert_eq!(got, want, "{what}");
    assert_eq!(
        pyjson::to_string(got, Some(1)),
        pyjson::to_string(want, Some(1)),
        "{what}: key order"
    );
}

fn chunk_values(chunks: Vec<RawChunk>) -> Value {
    Value::Array(chunks.into_iter().map(Value::Object).collect())
}

fn ids(raw: &Value) -> Vec<&str> {
    raw["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap())
        .collect()
}

/// 가짜 Slack이 method마다 주는 쪽이에요.
fn pages() -> Value {
    json!({
        "auth.test": {"url": "https://acme.slack.com/"},
        "users.list": {"members": [{"id": "U1", "profile": {"display_name": "민수"}}]},
        "conversations.list": {"channels": [{"id": "C1", "name": "dev-help", "is_member": true},
                                            {"id": "C2", "name": "infra", "is_member": true},
                                            {"id": "G1", "name": "leads", "is_member": true, "is_private": true}]},
        "conversations.history": {"messages": [{"ts": "1.0", "user": "U1", "text": "질문", "reply_count": 1}]},
        "conversations.replies": {"messages": [{"ts": "1.0", "text": "질문"}, {"ts": "2.0", "user": "U1", "text": "답"}]},
        "conversations.members": {"members": ["U1"]},
    })
}

/// `fail`에 든 (method, channel)이면 실패하고, 아니면 `{"ok": true, **pages[method]}`예요.
/// 부른 (method, channel)을 차례로 적어요.
struct FakeApi {
    pages: Value,
    fail: Vec<(&'static str, Option<&'static str>)>,
    calls: Mutex<Vec<(String, Option<String>)>>,
}

impl FakeApi {
    fn new(fail: &[(&'static str, Option<&'static str>)]) -> FakeApi {
        FakeApi::with_pages(pages(), fail)
    }

    fn with_pages(pages: Value, fail: &[(&'static str, Option<&'static str>)]) -> FakeApi {
        FakeApi {
            pages,
            fail: fail.to_vec(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, Option<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl SlackApi for FakeApi {
    fn call(&self, method: &str, params: &[(&str, String)]) -> Result<Value, SourceError> {
        let channel = params
            .iter()
            .find(|(k, _)| *k == "channel")
            .map(|(_, v)| v.clone());
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), channel.clone()));
        if self
            .fail
            .iter()
            .any(|(m, c)| *m == method && c.map(str::to_string) == channel)
        {
            return Err(SourceError::Other(format!(
                "slack {method}: internal_error"
            )));
        }
        let mut answer = Map::new();
        answer.insert("ok".into(), json!(true));
        for (k, v) in self.pages[method].as_object().unwrap() {
            answer.insert(k.clone(), v.clone());
        }
        Ok(Value::Object(answer))
    }
}

/// 지난 받기의 저장본이에요.
fn saved(dir: &Path) -> Value {
    let prev = json!({"team_url": "https://acme.slack.com/", "users": {"U1": "민수"}, "fetched_at": "2026-10-20T00:00:00Z",
                      "channels": [{"id": "C1", "name": "dev-help", "is_private": false, "members": null,
                                    "messages": [{"ts": "0.5", "user": "U1", "text": "지난번 글"}], "replies": {}}]});
    write(&dir.join("slack.json"), &prev);
    prev
}

#[test]
fn constants_match_python() {
    assert_eq!(API, "https://slack.com/api/");
    assert_eq!(DAYS, 180);
    assert_eq!(slack::BURST_GAP, 600.0);
    assert_eq!(slack::TRIES, 6);
    assert_eq!(slack::TIMEOUT, Duration::from_secs(30));
    let mut skip: Vec<&str> = slack::SKIP_SUBTYPES.to_vec();
    skip.sort();
    assert_eq!(
        skip,
        [
            "bot_message",
            "channel_archive",
            "channel_join",
            "channel_leave",
            "channel_name",
            "channel_purpose",
            "channel_topic",
            "channel_unarchive",
            "pinned_item",
            "reminder_add"
        ]
    );
}

#[test]
fn public_channel_failure_keeps_saved_copy_and_goes_on() {
    let tmp = tempfile::tempdir().unwrap();
    saved(tmp.path());
    let api = FakeApi::new(&[("conversations.history", Some("C1"))]);
    fetch_with(&api, tmp.path(), DAYS).unwrap();
    let raw = read(&tmp.path().join("slack.json"));
    let by: Map<String, Value> = raw["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_str().unwrap().to_string(), c.clone()))
        .collect();
    assert_eq!(by["C1"]["messages"][0]["text"], "지난번 글"); // 저장본이에요, 빠지지 않았어요
    assert_eq!(by["C2"]["messages"][0]["text"], "질문"); // 나머지는 받았어요
    assert_eq!(by["G1"]["members"], json!(["U1"]));
    assert_eq!(raw["failed"], json!(["#dev-help"]));
}

#[test]
fn public_channel_failure_without_saved_copy_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let api = FakeApi::new(&[("conversations.history", Some("C1"))]);
    fetch_with(&api, tmp.path(), DAYS).unwrap();
    let raw = read(&tmp.path().join("slack.json"));
    assert_eq!(ids(&raw), ["C2", "G1"]);
    assert_eq!(raw["failed"], json!(["#dev-help"]));
}

#[test]
fn private_channel_failure_drops_it_even_with_saved_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let mut prev = saved(tmp.path());
    prev["channels"].as_array_mut().unwrap().push(
        json!({"id": "G1", "name": "leads", "is_private": true, "members": ["U1", "U9"],
               "messages": [{"ts": "0.7", "user": "U9", "text": "비밀"}], "replies": {}}),
    );
    write(&tmp.path().join("slack.json"), &prev);
    let api = FakeApi::new(&[("conversations.members", Some("G1"))]);
    fetch_with(&api, tmp.path(), DAYS).unwrap();
    let raw = read(&tmp.path().join("slack.json"));
    assert_eq!(ids(&raw), ["C1", "C2"]); // 낡은 멤버 목록이 비공개 카드를 열면 안 돼요
    assert_eq!(raw["failed"], json!(["🔒leads"]));
}

#[test]
fn everything_failing_raises_so_the_saved_copy_is_used() {
    let tmp = tempfile::tempdir().unwrap();
    saved(tmp.path());
    let api = FakeApi::new(&[("conversations.list", None)]);
    assert!(fetch_with(&api, tmp.path(), DAYS).is_err());
    assert_eq!(
        read(&tmp.path().join("slack.json"))["fetched_at"],
        "2026-10-20T00:00:00Z"
    );
}

#[test]
fn all_channels_failing_raises() {
    let tmp = tempfile::tempdir().unwrap();
    let api = FakeApi::new(&[
        ("conversations.history", Some("C1")),
        ("conversations.history", Some("C2")),
        ("conversations.members", Some("G1")),
    ]);
    let err = fetch_with(&api, tmp.path(), DAYS).unwrap_err();
    assert_eq!(
        err.to_string(),
        "slack: every channel failed (#dev-help, #infra, 🔒leads)"
    );
    assert_eq!(err.kind(), Kind::Down); // 슬랙 실패는 모두 down이에요
    assert!(!tmp.path().join("slack.json").exists());
    assert!(!tmp.path().join("slack.json.tmp").exists());
}

#[test]
fn raw_file_is_written_whole() {
    let tmp = tempfile::tempdir().unwrap();
    // 앞에서 끊긴 받기가 남긴 임시 파일이 있어도 덮어쓰고 바꿔 넣어요
    fs::write(tmp.path().join("slack.json.tmp"), "{\"half").unwrap();
    let api = FakeApi::new(&[]);
    fetch_with(&api, tmp.path(), DAYS).unwrap();
    let raw = read(&tmp.path().join("slack.json")); // 온전한 JSON이에요
    assert_eq!(ids(&raw), ["C1", "C2", "G1"]);
    let left: Vec<String> = fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    // 키 순서와 바이트는 Python `json.dumps(data, ensure_ascii=False)` 모양 그대로예요(파일 형식은 바꾸지 않아요)
    assert_eq!(
        raw.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["team_url", "users", "channels", "failed", "fetched_at"]
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("slack.json")).unwrap(),
        pyjson::to_string(&raw, None)
    );
}

/// 슬랙 원본 예시예요.
fn slack_raw() -> Value {
    json!({
        "team_url": "https://acme.slack.com/",
        "users": {"U1": "민수", "U2": "하은"},
        "channels": [
            {"id": "C1", "name": "dev-help", "is_private": false, "members": null, "replies": {
                "1729500000.000100": [{"ts": "1729500060.000200", "user": "U2", "text": "<@U1> 먼저 `pnpm dev:doctor` 돌려 보세요"}]},
             "messages": [
                 {"ts": "1729500000.000100", "user": "U1", "text": "메일 탭 401 떠요 &lt;급함&gt;", "reply_count": 1, "thread_ts": "1729500000.000100"},
                 {"ts": "1729500300.000300", "user": "U2", "subtype": "channel_join", "text": "joined"},
                 {"ts": "1729503000.000400", "user": "U2", "text": "배포 끝났어요 <https://x.dev|대시보드>"},
                 {"ts": "1729503100.000500", "bot_id": "B1", "text": "CI passed"},
             ]},
            {"id": "G1", "name": "leads", "is_private": true, "members": ["U1"], "replies": {},
             "messages": [{"ts": "1729510000.000100", "user": "U1", "text": "채용 얘기"}]},
        ],
    })
}

#[test]
fn slack_threads_bursts_and_private_acl() {
    let tmp = tempfile::tempdir().unwrap();
    write(&tmp.path().join("slack.json"), &slack_raw());
    let chunks = chunks(tmp.path()).unwrap();
    let by: Map<String, Value> = chunks
        .iter()
        .map(|c| {
            (
                c["path"].as_str().unwrap().to_string(),
                Value::Object(c.clone()),
            )
        })
        .collect();
    let thread = &by["slack/C1/1729500000.000100"];
    let text = thread["text"].as_str().unwrap();
    assert!(text.contains("민수") && text.contains("메일 탭 401 떠요 <급함>"));
    assert!(text.contains("하은") && text.contains("@민수 먼저 `pnpm dev:doctor` 돌려 보세요"));
    assert_eq!(
        text.split('\n').nth(1).unwrap(),
        "meta: 스레드 · 민수, 하은 · 답글 1개"
    );
    assert_eq!(
        thread["url"],
        "https://acme.slack.com/archives/C1/p1729500000000100"
    );
    let burst = by["slack/C1/1729503000.000400"]["text"].as_str().unwrap();
    assert!(
        burst.contains("배포 끝났어요 대시보드")
            && !burst.contains("CI passed")
            && !burst.contains("joined")
    );
    let private = &by["slack/G1/1729510000.000100"];
    assert_eq!(private["acl"], json!(["slack:U1"]));
    let parsed: Vec<_> = chunks.iter().map(|c| chunk_of(c).unwrap()).collect();
    let ok = allowed(&parsed, Mask::Plans, &HashSet::new());
    let closed: Vec<&str> = parsed
        .iter()
        .zip(&ok)
        .filter(|(_, k)| !**k)
        .map(|(c, _)| c.path.as_str())
        .collect();
    assert_eq!(closed, ["slack/G1/1729510000.000100"]);
    let member = HashSet::from(["slack:U1".to_string()]);
    assert!(allowed(&parsed, Mask::Plans, &member).iter().all(|k| *k));
}

#[test]
fn slack_fetch_reads_only_member_channels() {
    let tmp = tempfile::tempdir().unwrap();
    let mut pages = pages();
    pages["conversations.list"] = json!({"channels": [
        {"id": "C1", "name": "dev-help", "is_member": true},
        {"id": "C2", "name": "random", "is_member": false},
        {"id": "G1", "name": "leads", "is_member": true, "is_private": true}]});
    let api = FakeApi::with_pages(pages, &[]);
    fetch_with(&api, tmp.path(), DAYS).unwrap();
    let raw = read(&tmp.path().join("slack.json"));
    let names: Vec<&str> = raw["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["dev-help", "leads"]);
    assert_eq!(
        raw["channels"][0]["replies"],
        json!({"1.0": [{"ts": "2.0", "user": "U1", "text": "답"}]})
    );
    assert_eq!(raw["channels"][0]["members"], Value::Null);
    assert_eq!(raw["channels"][1]["members"], json!(["U1"]));
    assert!(
        !api.calls()
            .contains(&("conversations.history".into(), Some("C2".into())))
    );
}

#[test]
fn slack_chunks_match_python_golden() {
    let golden = fixture("slack_chunks.json");
    let tmp = tempfile::tempdir().unwrap();
    write(&tmp.path().join("slack.json"), &golden["raw"]);
    assert_same_json(
        &chunk_values(chunks(tmp.path()).unwrap()),
        &golden["chunks"],
        "chunks",
    );
    let docs = documents(&golden["raw"]).unwrap();
    let docs = Value::Array(docs.iter().map(|d| d.to_value()).collect());
    assert_same_json(&docs, &golden["documents"], "documents");
    let users = golden["raw"]["users"].as_object().unwrap();
    for case in golden["clean"].as_array().unwrap() {
        let (text, want) = (case[0].as_str().unwrap(), case[1].as_str().unwrap());
        assert_eq!(clean(text, users).unwrap(), want, "clean({text:?})");
    }
    // 원본 파일이 없으면 조각도 없어요
    let empty = tempfile::tempdir().unwrap();
    assert!(chunks(empty.path()).unwrap().is_empty());
}

/// golden의 가짜 Slack이에요: "method|channel|ts|cursor"로 답하고, 부른 (method, params)를 적어요(`oldest`는 따로).
struct GoldenApi {
    answers: Map<String, Value>,
    calls: Mutex<Vec<Value>>,
    oldest: Mutex<Vec<String>>,
}

impl SlackApi for GoldenApi {
    fn call(&self, method: &str, params: &[(&str, String)]) -> Result<Value, SourceError> {
        let get = |k: &str| {
            params
                .iter()
                .find(|(n, _)| *n == k)
                .map_or("", |(_, v)| v.as_str())
        };
        let key = [method, get("channel"), get("ts"), get("cursor")].join("|");
        let shown: Vec<Value> = params
            .iter()
            .map(|(k, v)| {
                if *k == "oldest" {
                    self.oldest.lock().unwrap().push(v.clone());
                    json!([k, "<oldest>"])
                } else {
                    json!([k, v])
                }
            })
            .collect();
        self.calls.lock().unwrap().push(json!([method, shown]));
        let mut answer = Map::new();
        answer.insert("ok".into(), json!(true));
        for (k, v) in self.answers[&key].as_object().unwrap() {
            answer.insert(k.clone(), v.clone());
        }
        Ok(Value::Object(answer))
    }
}

#[test]
fn slack_fetch_writes_what_python_writes() {
    let golden = fixture("slack_chunks.json");
    let case = &golden["fetch"];
    let api = GoldenApi {
        answers: case["answers"].as_object().unwrap().clone(),
        calls: Mutex::new(Vec::new()),
        oldest: Mutex::new(Vec::new()),
    };
    let tmp = tempfile::tempdir().unwrap();
    let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    fetch_with(&api, tmp.path(), DAYS).unwrap();
    let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    // 부른 차례와 인자가 같아요(목록의 다음 쪽은 앞 쪽의 채널을 다 받은 뒤에 물어요)
    assert_eq!(Value::Array(api.calls.into_inner().unwrap()), case["calls"]);
    // 파일은 fetched_at 말고 바이트까지 같아요
    let text = fs::read_to_string(tmp.path().join("slack.json")).unwrap();
    let at = regex::Regex::new(r#""fetched_at": "(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ)""#).unwrap();
    assert!(at.is_match(&text), "{text}");
    let masked = at.replace(&text, r#""fetched_at": "<now>""#);
    assert_eq!(masked, case["written"].as_str().unwrap());
    // oldest는 지금 − 180일의 Python `str(float)`이에요
    let oldest = api.oldest.into_inner().unwrap();
    assert_eq!(oldest.len(), 3); // history: C1 두 쪽, G1 한 쪽
    assert!(oldest.iter().all(|o| *o == oldest[0]));
    let v: f64 = oldest[0].parse().unwrap();
    assert_eq!(pyjson::to_string(&json!(v), None), oldest[0]);
    let days = Duration::from_secs(DAYS as u64 * 86_400).as_secs_f64();
    assert!(
        before.as_secs_f64() - days - 1.0 <= v && v <= after.as_secs_f64() - days + 1.0,
        "{v}"
    );
}

#[test]
fn html_entities_unescape_like_python() {
    for (text, want) in [
        ("&amp;lt;", "&lt;"),
        ("&lt;급함&gt;", "<급함>"),
        ("&copy x", "© x"),
        ("&notit;", "¬it;"),
        ("&#x80;", "€"),
        ("&#39;", "'"),
        ("&amp", "&"),
        ("&ampx", "&x"),
        ("&unknown;", "&unknown;"),
        ("&#0;", "\u{fffd}"),
    ] {
        assert_eq!(py::html_unescape(text), want, "{text:?}");
    }
    // Python `html.unescape`가 아는 이름 붙은 참조 전부(뒤에 "x"가 붙은 것도)와 숫자 참조의 경계예요
    let cases = fixture("slack_unescape.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() > 4000);
    for case in cases {
        let (text, want) = (case[0].as_str().unwrap(), case[1].as_str().unwrap());
        assert_eq!(py::html_unescape(text), want, "{text:?}");
    }
}

#[test]
fn documents_fail_where_python_raises() {
    let base = slack_raw();
    let broken = |path: &[&str], v: Value| {
        let mut raw = base.clone();
        let mut at = &mut raw;
        for p in &path[..path.len() - 1] {
            at = match p.parse::<usize>() {
                Ok(i) => &mut at[i],
                Err(_) => &mut at[*p],
            };
        }
        at[path[path.len() - 1]] = v;
        raw
    };
    let cases = [
        ("no users", {
            let mut raw = base.clone();
            raw.as_object_mut().unwrap().shift_remove("users");
            raw
        }),
        (
            "a mentioned name that is not text",
            broken(&["users", "U1"], Value::Null),
        ),
        (
            "a private channel without members",
            broken(&["channels", "1", "members"], Value::Null),
        ),
        ("a kept message without ts", {
            let mut raw = base.clone();
            raw["channels"][0]["messages"][2]
                .as_object_mut()
                .unwrap()
                .shift_remove("ts");
            raw
        }),
        (
            "a ts that is not a number",
            broken(&["channels", "1", "messages", "0", "ts"], json!("soon")),
        ),
        (
            "a ts that is not finite",
            broken(&["channels", "1", "messages", "0", "ts"], json!("nan")),
        ),
        (
            "a ts past year 9999",
            broken(&["channels", "1", "messages", "0", "ts"], json!("1e20")),
        ),
        (
            "a text that is not text",
            broken(&["channels", "1", "messages", "0", "text"], json!(5)),
        ),
        (
            "an unhashable subtype",
            broken(&["channels", "1", "messages", "0", "subtype"], json!(["x"])),
        ),
        (
            "an unhashable user",
            broken(
                &["channels", "1", "messages", "0", "user"],
                json!({"id": "U1"}),
            ),
        ),
    ];
    for (what, raw) in cases {
        match documents(&raw) {
            Err(SourceError::Json(m)) => {
                // 글은 오류에 넣지 않아요
                assert!(!m.contains("채용") && !m.contains("메일"), "{what}: {m}");
            }
            other => panic!("{what}: {other:?}"),
        }
    }
    // 실패하지 않고 지나가는 것: 남기지 않는 글의 이상한 ts, 모르는 사람, 숫자 사용자
    let fine = [
        broken(&["channels", "0", "messages", "3", "ts"], json!("CI")),
        broken(&["channels", "1", "messages", "0", "user"], json!("U404")),
        broken(&["channels", "1", "messages", "0", "user"], json!(7)),
    ];
    for raw in fine {
        documents(&raw).unwrap();
    }
}

#[test]
fn token_comes_from_the_config_file() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = token(tmp.path()).unwrap_err();
    assert_eq!(
        missing.to_string(),
        "no Slack token: ~/.config/slack/config.json (docs/05)"
    );
    fs::create_dir_all(tmp.path().join("slack")).unwrap();
    let config = tmp.path().join("slack/config.json");
    fs::write(&config, r#"{"me": "U1"}"#).unwrap();
    assert!(token(tmp.path()).is_err());
    fs::write(&config, r#"{"bot_token": "xoxb-secret", "me": "U1"}"#).unwrap();
    assert_eq!(token(tmp.path()).unwrap(), "xoxb-secret");
    // 토큰은 Debug에 나오지 않아요
    let http = SlackHttp::new(API, "xoxb-secret");
    assert!(!format!("{http:?}").contains("xoxb-secret"));
    // 토큰이 없으면 아무것도 부르지 않고 실패해요
    let none = tempfile::tempdir().unwrap();
    let source = SlackSource {
        config_dir: none.path().to_path_buf(),
        base: "http://127.0.0.1:9/api/".into(),
    };
    let raw = tempfile::tempdir().unwrap();
    let err = source.fetch(raw.path()).unwrap_err();
    assert!(err.to_string().starts_with("no Slack token"));
    assert!(fs::read_dir(raw.path()).unwrap().next().is_none());
}

/// 로컬 HTTP 가짜의 답 하나예요: (상태, 머리, 몸통).
type Answer = (u16, Vec<(&'static str, &'static str)>, &'static str);

/// 로컬 HTTP 가짜예요: 요청마다 연결 하나로 차례대로 답하고(`Connection: close`), 받은 요청 머리를 모아요.
fn serve(answers: Vec<Answer>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/api/", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for (status, headers, body) in answers {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                head.push_str(&line);
            }
            log.lock().unwrap().push(head);
            let mut out = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                body.len()
            );
            for (k, v) in headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            out.push_str(body);
            let mut stream = stream;
            stream.write_all(out.as_bytes()).unwrap();
        }
    });
    (base, seen)
}

#[test]
fn slack_http_waits_out_rate_limits_and_reports_errors() {
    let (base, seen) = serve(vec![
        (429, vec![("Retry-After", "0")], ""),
        (
            200,
            vec![("Content-Type", "application/json")],
            r#"{"ok": false, "error": "invalid_auth"}"#,
        ),
    ]);
    let http = SlackHttp::new(&base, "xoxb-test");
    let start = Instant::now();
    let err = http.call("auth.test", &[]).unwrap_err();
    assert_eq!(err.to_string(), "slack auth.test: invalid_auth");
    assert_eq!(err.kind(), Kind::Down);
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "{:?}",
        start.elapsed()
    ); // Retry-After + 1 s
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    for head in &seen {
        assert!(
            head.starts_with("GET /api/auth.test HTTP/1.1\r\n"),
            "{head}"
        );
        let auth = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
            .unwrap();
        assert_eq!(auth, "authorization: Bearer xoxb-test");
    }
    assert!(!format!("{err:?}").contains("xoxb-test"));
}

#[test]
fn slack_http_sends_params_in_order_and_fails_like_python() {
    // 쿼리는 받은 차례 그대로예요(limit, 받은 인자, cursor)
    let (base, seen) = serve(vec![(
        200,
        vec![],
        r#"{"ok": true, "messages": [], "response_metadata": {"next_cursor": ""}}"#,
    )]);
    let http = SlackHttp::new(&base, "xoxb-test");
    let params = [
        ("limit", "200".to_string()),
        ("channel", "C1".to_string()),
        ("oldest", "1713000000.123456".to_string()),
        ("cursor", "dXNlcjpVMDI=".to_string()),
    ];
    let got = http.call("conversations.history", &params).unwrap();
    assert_eq!(got["ok"], true);
    let head = seen.lock().unwrap()[0].clone();
    assert!(
        head.starts_with(
            "GET /api/conversations.history?limit=200&channel=C1&oldest=1713000000.123456&cursor=dXNlcjpVMDI%3D HTTP/1.1"
        ),
        "{head}"
    );

    // 429가 여섯 번이면 rate limited예요(Retry-After −1이면 쉬지 않아요: int(−1) + 1 = 0)
    let (base, seen) = serve(vec![(429, vec![("Retry-After", "-1")], ""); 6]);
    let err = SlackHttp::new(&base, "t")
        .call("users.list", &[])
        .unwrap_err();
    assert_eq!(err.to_string(), "slack users.list: rate limited");
    assert_eq!(seen.lock().unwrap().len(), 6);
    // 실패하는 답: 쉴 수 없는 Retry-After, JSON이 아닌 몸통, 객체가 아닌 JSON
    for (status, headers, body) in [
        (429, vec![("Retry-After", "-2")], ""),
        (429, vec![("Retry-After", "soon")], ""),
        (200, vec![], "<html>"),
        (502, vec![], "[1]"),
    ] {
        let (base, _) = serve(vec![(status, headers, body)]);
        let err = SlackHttp::new(&base, "t")
            .call("auth.test", &[])
            .unwrap_err();
        assert!(err.to_string().starts_with("slack auth.test: "), "{err}");
        assert_eq!(err.kind(), Kind::Down);
    }
    // ok가 없으면 오류 글은 None이에요
    let (base, _) = serve(vec![(200, vec![], r#"{"warning": "x"}"#)]);
    let err = SlackHttp::new(&base, "t")
        .call("auth.test", &[])
        .unwrap_err();
    assert_eq!(err.to_string(), "slack auth.test: None");
    // 닿지 않는 곳도 down이에요
    let err = SlackHttp::new("http://127.0.0.1:9/api/", "t")
        .call("auth.test", &[])
        .unwrap_err();
    assert_eq!(err.kind(), Kind::Down);
    assert!(err.to_string().starts_with("slack auth.test: "), "{err}");
}

#[test]
fn fake_slack_writes_or_fails_and_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("slack");
    let fake = FakeSlack::writes(slack_raw());
    fake.fetch(&dir).unwrap();
    fake.fetch(&dir).unwrap();
    assert_eq!(fake.calls(), 2);
    assert_eq!(read(&dir.join("slack.json")), slack_raw());
    let fails = FakeSlack::fails("slack: every channel failed (#dev-help)");
    let err = fails.fetch(&dir).unwrap_err();
    assert_eq!(err.to_string(), "slack: every channel failed (#dev-help)");
    assert_eq!(fails.calls(), 1);
}
