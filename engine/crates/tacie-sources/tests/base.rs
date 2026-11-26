//! Error kinds, asking for the gh token, Python text rules, saving JSON files, swapping in folders.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;
use tacie_sources::error::{Kind, SourceError};
use tacie_sources::github::gh_token::{GhToken, NOT_LOGGED_IN, SystemGhToken};
use tacie_sources::py;
use tacie_sources::store::{self, Stored};

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    out.sort();
    out
}

#[test]
fn source_errors_tell_their_kind_and_detail() {
    let busy = SourceError::service(Kind::Busy, "GitHub's rate limit stopped the reconcile");
    assert_eq!(busy.kind(), Kind::Busy);
    assert_eq!(busy.detail(), "GitHub's rate limit stopped the reconcile");
    assert_eq!(busy.to_string(), busy.detail());
    let key = SourceError::service(Kind::Key, NOT_LOGGED_IN);
    assert_eq!(key.kind(), Kind::Key);
    assert_eq!(key.detail(), "gh is not logged in");
    for other in [
        SourceError::Json("bad".into()),
        SourceError::Other("disk".into()),
    ] {
        assert_eq!(other.kind(), Kind::Down);
    }
    assert_eq!(SourceError::Json("bad".into()).detail(), "bad");
    let from_io: SourceError = std::io::Error::other("disk full").into();
    assert_eq!(from_io.kind(), Kind::Down);
    assert!(from_io.detail().contains("disk full"));
    let from_anyhow: SourceError = anyhow::anyhow!("bad graph").into();
    assert_eq!(from_anyhow.detail(), "bad graph");
    assert_eq!(
        serde_json::to_value(Kind::Network).unwrap(),
        json!("network")
    );
}

#[test]
fn kind_names_match_serde() {
    // 로그 줄과 응답의 `error_kind`가 같은 글자예요
    for (kind, name) in [
        (Kind::Key, "key"),
        (Kind::Credit, "credit"),
        (Kind::Network, "network"),
        (Kind::Busy, "busy"),
        (Kind::Down, "down"),
    ] {
        assert_eq!(kind.as_str(), name);
        assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
    }
}

#[test]
fn read_json_tells_missing_from_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.json");
    assert_eq!(store::read_json(&p).unwrap(), Stored::Missing);
    fs::write(&p, "{").unwrap();
    assert_eq!(store::read_json(&p).unwrap(), Stored::Unreadable);
    fs::write(&p, b"{\"x\": \"\xff\"}").unwrap(); // 깨진 UTF-8도 못 읽는 JSON이에요
    assert_eq!(store::read_json(&p).unwrap(), Stored::Unreadable);
    fs::write(&p, r#"{"x": 1}"#).unwrap();
    assert_eq!(
        store::read_json(&p).unwrap(),
        Stored::Value(json!({"x": 1}))
    );
    // 폴더는 깨진 파일이 아니라 IO 오류예요
    assert!(store::read_json(dir.path()).is_err());
}

#[test]
fn write_json_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("a").join("x.json");
    store::write_json(&p, &json!({"a": 1}), None).unwrap(); // 부모 폴더를 만들어요
    store::write_json(&p, &json!({"a": 2}), None).unwrap();
    assert_eq!(fs::read_to_string(&p).unwrap(), r#"{"a": 2}"#);
    assert_eq!(names(p.parent().unwrap()), ["x.json"]);

    // 바꿔 넣을 자리에 비어 있지 않은 폴더가 있으면 실패하고, 폴더는 그대로이고 임시 파일은 남지 않아요
    let q = dir.path().join("b").join("x.json");
    fs::create_dir_all(q.join("inside")).unwrap();
    assert!(store::write_json(&q, &json!({"a": 1}), None).is_err());
    assert!(q.join("inside").is_dir());
    assert_eq!(names(q.parent().unwrap()), ["x.json"]);
}

#[test]
fn write_json_with_indent_matches_python() {
    // json.dumps(v, ensure_ascii=False[, indent=1])를 uv run python -I로 뽑은 바이트예요
    let v = json!({
        "a": [1, 2.5, {"b": null, "c": []}, {}],
        "k": "한\u{1}\u{7f}\"\\ \u{2028}",
        "f": 1e16,
        "g": 1e-5,
        "t": true
    });
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("v.json");
    store::write_json(&p, &v, Some(1)).unwrap();
    assert_eq!(
        fs::read_to_string(&p).unwrap(),
        "{\n \"a\": [\n  1,\n  2.5,\n  {\n   \"b\": null,\n   \"c\": []\n  },\n  {}\n ],\n \
         \"k\": \"한\\u0001\u{7f}\\\"\\\\ \u{2028}\",\n \"f\": 1e+16,\n \"g\": 1e-05,\n \"t\": true\n}"
    );
    store::write_json(&p, &v, None).unwrap();
    assert_eq!(
        fs::read_to_string(&p).unwrap(),
        "{\"a\": [1, 2.5, {\"b\": null, \"c\": []}, {}], \"k\": \"한\\u0001\u{7f}\\\"\\\\ \u{2028}\", \
         \"f\": 1e+16, \"g\": 1e-05, \"t\": true}"
    );
}

#[test]
fn swap_in_replaces_live_and_removes_old() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("raw");
    let new = dir.path().join("raw+new");
    fs::create_dir_all(&live).unwrap();
    fs::write(live.join("old.txt"), "old").unwrap();
    fs::create_dir_all(&new).unwrap();
    fs::write(new.join("new.txt"), "new").unwrap();
    fs::create_dir_all(dir.path().join("raw+old/stale")).unwrap(); // 지난번에 끊긴 바꿔 넣기가 남긴 것
    store::swap_in(&new, &live).unwrap();
    assert_eq!(names(&live), ["new.txt"]);
    assert_eq!(names(dir.path()), ["raw"]);

    // 살아 있는 자리가 없어도 돼요
    let fresh = dir.path().join("fresh");
    let fresh_new = dir.path().join("fresh+new");
    fs::create_dir_all(&fresh_new).unwrap();
    fs::write(fresh_new.join("a"), "a").unwrap();
    store::swap_in(&fresh_new, &fresh).unwrap();
    assert_eq!(names(&fresh), ["a"]);
    assert_eq!(names(dir.path()), ["fresh", "raw"]);
}

#[test]
fn python_text_rules() {
    assert_eq!(
        py::splitlines("a\r\nb\rc\x0bd\x0ce\x1cf\x1dg\x1eh\u{85}i\u{2028}j\u{2029}k\x1fl\n"),
        ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k\x1fl"]
    );
    assert_eq!(py::splitlines(""), Vec::<&str>::new());
    assert_eq!(py::splitlines("\n"), [""]);
    assert_eq!(py::splitlines("a\n\nb"), ["a", "", "b"]);
    assert_eq!(py::splitlines("a\r\r\nb\r"), ["a", "", "b"]);
    assert_eq!(py::strip("\x1f\x1c 한 \u{3000}\u{85}"), "한");
    assert_eq!(py::strip(" \u{feff}x "), "\u{feff}x"); // BOM은 공백이 아니에요
    assert_eq!(py::strip("\u{a0}\u{200b}\u{a0}"), "\u{200b}");
    assert_eq!(
        py::decode_ignore(b"ok\xff\xfe\xc3\x28end\xe2\x82"),
        "ok(end"
    );
    assert_eq!(py::decode_ignore(b"\xed\xa0\x80a\xe0\x80\xafb"), "ab"); // 서로게이트, 너무 긴 인코딩
    assert_eq!(py::take_chars("가나다", 2), "가나");
    assert_eq!(py::take_chars("가나다", 3), "가나다");
    assert_eq!(py::take_chars("가나다", 9), "가나다");
    assert_eq!(py::take_chars("", 0), "");

    for falsy in [
        json!(null),
        json!(false),
        json!(0),
        json!(0.0),
        json!(""),
        json!([]),
        json!({}),
    ] {
        assert!(!py::truthy(&falsy), "{falsy}");
    }
    for truthy in [
        json!(true),
        json!(1),
        json!(-0.5),
        json!("0"),
        json!([0]),
        json!({"a": null}),
    ] {
        assert!(py::truthy(&truthy), "{truthy}");
    }

    // Python str.isspace()가 참인 글자 전부예요(uv run python -I로 뽑았어요)
    let python: Vec<u32> = vec![
        0x9, 0xa, 0xb, 0xc, 0xd, 0x1c, 0x1d, 0x1e, 0x1f, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001,
        0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029,
        0x202f, 0x205f, 0x3000,
    ];
    let ours: Vec<u32> = (0..=0x10ffff)
        .filter_map(char::from_u32)
        .filter(|&c| py::is_space(c))
        .map(u32::from)
        .collect();
    assert_eq!(ours, python);

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("t.txt");
    fs::write(&f, b"\xef\xbb\xbfa\xff\r\nb").unwrap();
    assert_eq!(py::read_text(&f).unwrap(), "\u{feff}a\r\nb");
    assert!(py::read_text(&dir.path().join("missing")).is_err());
}

#[test]
fn http_errors_are_classified() {
    use std::collections::HashMap;
    use tacie_sources::github::http::kind_of;

    let none = HashMap::new();
    let spent: HashMap<String, String> =
        [("x-ratelimit-remaining".to_string(), "0".to_string())].into();
    let msg = |m: &str| json!({"message": m});
    type Case<'a> = (
        u16,
        &'a HashMap<String, String>,
        Option<serde_json::Value>,
        Option<(Kind, &'a str)>,
    );
    let cases: Vec<Case> = vec![
        (200, &none, None, None),
        (201, &none, None, None),
        (
            401,
            &none,
            Some(msg("Bad credentials")),
            Some((Kind::Key, "GitHub answered HTTP 401 (bad credentials)")),
        ),
        (
            403,
            &spent,
            None,
            Some((Kind::Busy, "GitHub answered HTTP 403 (rate limit)")),
        ),
        (
            403,
            &none,
            Some(msg("You have exceeded a secondary rate limit")),
            Some((Kind::Busy, "GitHub answered HTTP 403 (rate limit)")),
        ),
        (
            403,
            &none,
            Some(msg(
                "Resource protected by organization SAML enforcement. You must grant your OAuth token access to acme.",
            )),
            Some((Kind::Key, "GitHub answered HTTP 403 (SAML enforcement)")),
        ),
        (
            403,
            &none,
            Some(msg(
                "Although you appear to have the correct authorization credentials, the `acme` organization has enabled OAuth App access restrictions",
            )),
            Some((
                Kind::Key,
                "GitHub answered HTTP 403 (OAuth App access restrictions)",
            )),
        ),
        (
            403,
            &none,
            Some(msg("Resource not accessible by integration")),
            Some((Kind::Key, "GitHub answered HTTP 403 (not accessible)")),
        ),
        (
            403,
            &none,
            Some(msg("Forbidden acme")),
            Some((Kind::Key, "GitHub answered HTTP 403")),
        ),
        (
            403,
            &none,
            None,
            Some((Kind::Key, "GitHub answered HTTP 403")),
        ),
        (
            404,
            &none,
            Some(msg("Not Found")),
            Some((Kind::Key, "GitHub answered HTTP 404")),
        ),
        (
            410,
            &none,
            None,
            Some((Kind::Key, "GitHub answered HTTP 410")),
        ),
        (
            422,
            &none,
            Some(msg("No commit found for SHA: x")),
            Some((Kind::Down, "GitHub answered HTTP 422")),
        ),
        (
            429,
            &none,
            None,
            Some((Kind::Busy, "GitHub answered HTTP 429 (rate limit)")),
        ),
        (
            500,
            &none,
            None,
            Some((Kind::Down, "GitHub answered HTTP 500")),
        ),
        (
            502,
            &none,
            None,
            Some((Kind::Down, "GitHub answered HTTP 502")),
        ),
        (
            304,
            &none,
            None,
            Some((Kind::Down, "GitHub answered HTTP 304")),
        ),
    ];
    for (status, headers, body, want) in cases {
        let got = kind_of(status, headers, body.as_ref());
        let want = want.map(|(k, m)| (k, m.to_string()));
        assert_eq!(got, want, "{status} {body:?}");
    }
}

// ---- gh 모드의 토큰(`github::gh_token`, 스펙 6-3) ----

fn gh_token_with(script: &str, timeout: Duration) -> Result<String, SourceError> {
    SystemGhToken::with_program("sh", &["-c", script], timeout).token()
}

#[test]
fn gh_token_times_out_and_kills() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!("echo $$ > '{}'; exec sleep 30", pid_file.display());
    let start = Instant::now();
    let e = gh_token_with(&script, Duration::from_millis(300)).unwrap_err();
    assert_eq!(
        (e.kind(), e.detail()),
        (Kind::Key, NOT_LOGGED_IN.to_string())
    );
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
    let pid = fs::read_to_string(&pid_file).unwrap().trim().to_string();
    let alive = Command::new("kill").args(["-0", &pid]).output().unwrap();
    assert!(!alive.status.success(), "the gh child {pid} still runs");
}

#[test]
fn gh_token_gives_no_stdin() {
    // stdin이 비어 있어서 읽는 프로그램도 기다리지 않고 끝나요
    let got = gh_token_with("cat; echo gho_after_stdin", Duration::from_secs(3)).unwrap();
    assert_eq!(got, "gho_after_stdin");
}

#[test]
fn gh_token_strips_output() {
    let got = gh_token_with("printf '  gho_abc \\n\\n'", Duration::from_secs(3)).unwrap();
    assert_eq!(got, "gho_abc");
    // 기본은 `gh auth token`, 5 s예요
    let real = format!("{:?}", SystemGhToken::new());
    assert!(
        real.contains("\"gh\"") && real.contains("\"auth\", \"token\""),
        "{real}"
    );
    assert!(real.contains("5s"), "{real}");
}

#[test]
fn gh_token_failure_is_key() {
    let key = |r: Result<String, SourceError>| {
        let e = r.unwrap_err();
        assert_eq!(
            (e.kind(), e.detail()),
            (Kind::Key, NOT_LOGGED_IN.to_string())
        );
        // gh가 찍은 글은 오류에 넣지 않아요
        assert!(!format!("{e:?}").contains("gho_"), "{e:?}");
    };
    key(gh_token_with(
        "echo gho_secret; exit 1",
        Duration::from_secs(3),
    )); // 0이 아닌 종료
    key(gh_token_with("printf '  \\n'", Duration::from_secs(3))); // 빈 출력
    key(gh_token_with("kill -9 $$", Duration::from_secs(3))); // 시그널
    key(SystemGhToken::with_program(
        "/nonexistent/gh",
        &["auth", "token"],
        Duration::from_secs(3),
    )
    .token()); // gh가 없어요
}

#[test]
fn gh_token_does_not_wait_for_a_child_holding_the_pipe() {
    // gh가 끝나도 남은 자식이 stdout을 쥐고 있으면 읽기는 같은 시각까지만 기다려요
    let start = Instant::now();
    let e = gh_token_with("(exec sleep 30) & echo gho_x", Duration::from_millis(400)).unwrap_err();
    assert_eq!(e.kind(), Kind::Key);
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );
}
