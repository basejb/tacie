//! The engine does not launch processes (the first two layers of spec 6-3 and 7-4). token and none modes never ask
//! gh for a token on any path, and the only place in the source that launches a process is `gh auth token` in gh
//! mode (`github/gh_token.rs`).

#[allow(dead_code)] // 이 파일은 샌드박스의 일부만 써요
mod common {
    pub mod fakes;
    pub mod fixture;
    pub mod sandbox;
}

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use axum::http::StatusCode;
use common::sandbox::{Sandbox, finish};
use regex::Regex;
use serde_json::json;
use tacie_sources::testing::github::{FakeGitHub, Reply};

/// token 모드의 엔진에 앱이 보내는 모든 GitHub·색인 경로를 보내요. gh 토큰은 부르면 패닉해요: 갱신 스레드의
/// 패닉은 `failed`로 끝나서, 모든 갱신이 `done`(또는 key 실패)인지도 봐요.
#[test]
fn token_mode_never_asks_gh() {
    let s = Sandbox::new();
    let asked = s.gh_token.calls();
    s.panic_on_token(true);
    let (status, got) = s.call(
        "POST",
        "/github/credentials",
        &json!({"mode": "token", "token": FakeGitHub::TOKEN}).to_string(),
    );
    assert_eq!(
        (status, got),
        (
            StatusCode::OK,
            json!({"connected": true, "login": "basejb"})
        )
    );
    s.gh.route(
        "user/repos?affiliation=owner,collaborator,organization_member&sort=pushed",
        Reply::Json(
            json!([{"full_name": "o/old", "private": true, "pushed_at": "2026-10-24T00:00:00Z",
                            "default_branch": "main", "owner": {"login": "o", "type": "User"}}]),
        ),
    );
    s.gh.route("user/orgs", Reply::Json(json!([])));
    s.gh.route(
        "repos/o/old/branches",
        Reply::Json(json!([{"name": "main"}])),
    );

    let (status, got) = s.call("GET", "/index", "");
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["connected"], true, "{got}");

    let (status, got) = s.call(
        "POST",
        "/index/rebuild",
        &json!({"repo": "o/new", "branch": "main"}).to_string(),
    );
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(finish(&s.r)["state"], "done");
    let (status, got) = s.call("POST", "/index/refresh", "");
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(finish(&s.r)["state"], "done");
    let (status, got) = s.call("POST", "/index/check", "");
    assert_eq!(status, StatusCode::OK, "{got}");
    let (status, got) = s.call("POST", "/index/clear", "");
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(finish(&s.r)["state"], "idle");

    let (status, got) = s.call("GET", "/github/repos", "");
    assert_eq!(status, StatusCode::OK, "{got}");
    assert_eq!(got["repos"][0]["full_name"], "o/old", "{got}");
    let (status, got) = s.call("GET", "/github/branches?repo=o/old", "");
    assert_eq!(
        (status, got),
        (
            StatusCode::OK,
            json!({"branches": ["main"], "default": "main"})
        )
    );

    let (status, got) = s.call("POST", "/github/credentials", r#"{"mode": "none"}"#);
    assert_eq!(
        (status, got),
        (StatusCode::OK, json!({"connected": false, "login": null}))
    );
    let (status, got) = s.call(
        "POST",
        "/index/rebuild",
        &json!({"repo": "o/old", "branch": "main"}).to_string(),
    );
    assert_eq!(status, StatusCode::OK, "{got}");
    let st = finish(&s.r);
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "key", "{st}");
    let (status, got) = s.call("POST", "/index/refresh", "");
    assert_eq!(status, StatusCode::OK, "{got}");
    let st = finish(&s.r);
    assert_eq!(st["state"], "failed", "{st}");
    assert_eq!(st["error_kind"], "key", "{st}");

    assert_eq!(s.gh_token.calls(), asked);
}

/// `dir` 아래 모든 `.rs` 파일이에요.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 프로세스를 띄울 수 있는 글인지예요. 주석 줄(`//`로 시작)은 빼고 파일 전체를 한 덩어리로 봐서, 여러 줄의
/// `use std::process::{\n Command,\n};`도 잡아요: `Command::new`, 그리고 `process` 경로(`std`·`tokio`, 중첩 `use`) 뒤(같은 `use`
/// 묶음 안 포함)의 `Command` 낱말이에요.
fn spawns(text: &str) -> bool {
    static NEW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bCommand\s*::\s*new\b").unwrap());
    static PATH: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\bprocess\s*::\s*(?:\{[^;]*?\bCommand\b|Command\b)").unwrap()
    });
    let code: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    NEW.is_match(&code) || PATH.is_match(&code)
}

#[test]
fn the_scan_sees_every_way_to_name_command() {
    for sample in [
        "use std::process::{\n    Command,\n};\n",
        "use std::process::{\n    Stdio,\n    Command as C,\n};\n",
        "use std::process::Command;\n",
        "let c = std::process::Command::new(\"git\");\n",
        "use tokio::process::Command;\n",
        "use std::{io, process::{Command, Stdio}};\n",
        "use std::process;\nprocess::Command::new(\"git\");\n",
        "let c = Command :: new(\"git\");\n",
    ] {
        assert!(spawns(sample), "{sample}");
    }
    for sample in [
        "std::process::exit(1);\n",
        "use std::process::ExitCode;\n",
        "// use std::process::Command;\n",
        "/// `Command::new`을 쓰지 않아요\n",
        "use std::process::{ExitCode, Stdio};\nstruct Command;\n",
    ] {
        assert!(!spawns(sample), "{sample}");
    }
}

/// 모든 crate의 `src`와 `build.rs`에서 프로세스를 띄울 수 있는 파일은 `gh_token.rs` 하나예요. 누가 git을 다시
/// 부르면 이 테스트가 먼저 깨져요.
#[test]
fn only_gh_token_spawns_processes() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = BTreeSet::new();
    let mut scanned = 0;
    for krate in fs::read_dir(&crates).unwrap() {
        let krate = krate.unwrap().path();
        let mut files = Vec::new();
        rust_files(&krate.join("src"), &mut files);
        if krate.join("build.rs").is_file() {
            files.push(krate.join("build.rs"));
        }
        for f in files {
            scanned += 1;
            if spawns(&fs::read_to_string(&f).unwrap()) {
                let rel = f.strip_prefix(&crates).unwrap();
                found.insert(rel.to_string_lossy().into_owned());
            }
        }
    }
    // tacie-mcp has no process to start (it only talks to the app's socket): its src must be among what is scanned
    assert!(
        crates.join("tacie-mcp/src/main.rs").is_file() && crates.join("tacie-mcp/src/socket.rs").is_file(),
        "tacie-mcp/src is not where the scan looks"
    );
    assert!(scanned > 30, "scanned only {scanned} files"); // 네 crate의 src를 다 봤어요
    assert_eq!(
        found,
        BTreeSet::from(["tacie-sources/src/github/gh_token.rs".to_string()])
    );
}
