//! The GitHub connection: modes, tokens, login, REST calls, repo and branch lists. GitHub is answered by `FakeGitHub`
//! (a loopback fake server), and the gh token by `FakeGhToken`/`PanicGhToken`. git is never launched.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tacie_sources::error::{Kind, SourceError};
use tacie_sources::github::gh_token::{GH_TOKEN_TIMEOUT, GhToken, NOT_LOGGED_IN};
use tacie_sources::github::http::{
    API, API_VERSION, CONNECT_TIMEOUT, REQUEST_TIMEOUT, TARBALL_TIMEOUT, USER_AGENT,
};
use tacie_sources::github::{
    self, CHECK_TIMEOUT, GH_RETRY, GRAPHQL_TRIES, GitHub, LOGIN_TIMEOUT, LoginState, Mode,
    PROBE_TIMEOUT, PROBE_WORKERS, Timeouts,
};
use tacie_sources::testing::github::{FakeGhToken, FakeGitHub, PanicGhToken, Reply};

const TOKEN: &str = FakeGitHub::TOKEN;

/// 연결이에요: gh 토큰은 부르면 패닉하고(token·none 모드), 다시 묻기 전에 쉬지 않아요.
fn connect(mode: &str, fake: &FakeGitHub) -> GitHub {
    GitHub::new(mode, &fake.url(), Arc::new(PanicGhToken))
        .unwrap()
        .with_pause(|_| {})
}

/// 가짜의 토큰을 가진 token 모드 연결이에요.
fn token(fake: &FakeGitHub) -> GitHub {
    let gh = connect("none", fake);
    gh.set_credentials("token", Some(TOKEN));
    gh
}

/// gh 모드 연결이에요: gh가 차례로 이렇게 답해요.
fn gh_mode(fake: &FakeGitHub, gh_token: &Arc<FakeGhToken>) -> GitHub {
    GitHub::new("gh", &fake.url(), gh_token.clone())
        .unwrap()
        .with_pause(|_| {})
}

fn names(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn is_key_error(r: Result<impl std::fmt::Debug, SourceError>) -> bool {
    matches!(r, Err(SourceError::Service { kind: Kind::Key, ref message }) if message == "GitHub is not connected")
}

fn http(code: u16, body: &str) -> Reply {
    Reply::Http {
        code,
        headers: vec![],
        body: body.as_bytes().to_vec(),
    }
}

/// 물은 (경로, 쿼리)예요.
fn asked(fake: &FakeGitHub) -> Vec<(String, String)> {
    fake.calls()
        .iter()
        .map(|c| (c.path.clone(), c.query.clone()))
        .collect()
}

#[test]
fn constants_match_python() {
    assert_eq!(LOGIN_TIMEOUT, Duration::from_secs(5));
    assert_eq!(PROBE_TIMEOUT, Duration::from_secs(8));
    assert_eq!(CHECK_TIMEOUT, Duration::from_secs(20));
    assert_eq!(PROBE_WORKERS, 4);
    assert_eq!(GRAPHQL_TRIES, 3);
    // REST(스펙 3절 12·16)
    assert_eq!(API, "https://api.github.com/");
    assert_eq!(USER_AGENT, concat!("Tacie/", env!("CARGO_PKG_VERSION")));
    assert_eq!(API_VERSION, "2022-11-28");
    assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(10));
    assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    assert_eq!(TARBALL_TIMEOUT, Duration::from_secs(300));
    assert_eq!(GH_TOKEN_TIMEOUT, Duration::from_secs(5));
    assert_eq!(
        Timeouts::default(),
        Timeouts {
            login: LOGIN_TIMEOUT,
            probe: PROBE_TIMEOUT,
            check: CHECK_TIMEOUT,
            request: REQUEST_TIMEOUT,
            gh_retry: GH_RETRY,
        }
    );
    assert_eq!(GH_RETRY, Duration::from_secs(60));
    // 주소가 아니면 만들지 않아요(down)
    let e = GitHub::new("gh", "not a url", Arc::new(PanicGhToken)).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
}

#[test]
fn github_without_connection_is_a_key_error() {
    let fake = FakeGitHub::start();
    let gh = connect("none", &fake);
    assert!(!gh.connected());
    assert!(!gh.connected_now());
    // 연결이 없으면 아무것도 묻지 않고 key예요
    assert!(is_key_error(gh.json("user", 3)));
    assert!(is_key_error(gh.status(
        "repos/o/n",
        &[],
        Some(CHECK_TIMEOUT)
    )));
    assert!(is_key_error(gh.pages("user/orgs")));
    assert!(is_key_error(gh.rest("repos/o/n/issues?state=all", None, 3)));
    assert!(is_key_error(gh.graphql("query{x}", "o", "n")));
    assert!(is_key_error(gh.repos()));
    assert!(is_key_error(gh.branches("o/n")));
    assert_eq!(
        gh.probe("orgs/x/repos?type=private&per_page=1"),
        (false, "GitHub is not connected".to_string())
    );
    assert_eq!(gh.login_state(), LoginState::Unknown);
    assert!(fake.calls().is_empty());
}

#[test]
fn login_is_none_when_not_connected() {
    let fake = FakeGitHub::start();
    let gh_token = Arc::new(FakeGhToken::gives(TOKEN));
    let gh = gh_mode(&fake, &gh_token);
    gh.set_credentials("none", None);
    assert_eq!(gh.login(), None);
    assert!(!gh.connected_now());
    assert!(
        fake.calls().is_empty(),
        "nothing is asked without a connection"
    );
    assert_eq!(gh_token.calls(), 0);
}

#[test]
fn github_login_is_none_without_gh() {
    // gh가 없거나 로그아웃했어요: 토큰이 없으니 GitHub에 묻지 않고, 연결은 거절(Refused)이에요
    let fake = FakeGitHub::start();
    let gh_token = Arc::new(FakeGhToken::fails());
    let gh = gh_mode(&fake, &gh_token);
    assert_eq!(gh.login(), None);
    assert_eq!(gh.login_state(), LoginState::Refused);
    assert!(gh.connected()); // 모드는 그대로예요(갱신은 key로 실패해요)
    assert!(!gh.connected_now());
    assert_eq!(gh_token.calls(), 1); // 거절은 이 세대 동안 기억해요
    let e = gh.json("repos/o/n", 3).unwrap_err();
    assert_eq!(
        (e.kind(), e.detail()),
        (Kind::Key, NOT_LOGGED_IN.to_string())
    );
    // gh_retry(60 s) 동안은 gh를 다시 띄우지 않고 바로 실패해요
    assert_eq!(gh_token.calls(), 1);
    assert!(!gh.connected_now());
    assert_eq!(gh_token.calls(), 1);
    assert!(fake.calls().is_empty());

    // gh에 다시 로그인하면 연결을 다시 고른 뒤(새 세대) 찾아요
    let gh_token = Arc::new(FakeGhToken::new(vec![None, Some(TOKEN)]));
    fake.login(Reply::Json(json!({"login": "basejb"})));
    let gh = gh_mode(&fake, &gh_token);
    assert_eq!(gh.login(), None);
    gh.set_credentials("gh", None);
    assert_eq!(gh.login().as_deref(), Some("basejb"));
    assert!(gh.connected_now());
    let c = &fake.calls()[0];
    assert_eq!((c.path.as_str(), c.has_token), ("user", true));
}

/// gh 실패(없음, 로그아웃, 느림)의 거절은 `gh_retry` 동안만이에요: 그 뒤에는 gh에 다시 묻고, 로그인했으면
/// 연결을 다시 고르지 않아도 연결이에요.
#[test]
fn a_gh_failure_is_asked_again_after_a_while() {
    let fake = FakeGitHub::start();
    fake.login(Reply::Json(json!({"login": "basejb"})));
    let gh_token = Arc::new(FakeGhToken::new(vec![None, Some(TOKEN)]));
    // 테스트가 넘기는 시계예요(기다리지 않아요)
    let start = std::time::Instant::now();
    let ahead = Arc::new(AtomicU64::new(0));
    let clock = ahead.clone();
    let gh = gh_mode(&fake, &gh_token).with_clock(Arc::new(move || {
        start + Duration::from_secs(clock.load(Ordering::SeqCst))
    }));
    assert!(!gh.connected_now());
    ahead.store(59, Ordering::SeqCst);
    assert!(!gh.connected_now()); // 60 s가 아직이에요
    assert_eq!(gh_token.calls(), 1);
    ahead.store(61, Ordering::SeqCst);
    assert!(gh.connected_now());
    assert_eq!(gh.login().as_deref(), Some("basejb"));
    assert_eq!(gh_token.calls(), 2);
    assert_eq!(fake.calls().len(), 1); // GET user 한 번
}

/// gh에 묻는 사이에 연결을 다시 고르면, 기다리던 스레드는 새 연결로 다시 해요(잠금을 쥔 채 다시 하지 않아요).
#[test]
fn a_credentials_switch_while_gh_is_asked_does_not_deadlock() {
    use std::sync::mpsc::{Receiver, Sender, channel};
    /// 첫 물음은 들어왔다고 알리고, 테스트가 놓아 줄 때까지 기다려요(시계 여유에 기대지 않아요).
    struct SlowGh {
        calls: AtomicUsize,
        entered: Mutex<Sender<()>>,
        release: Mutex<Receiver<()>>,
    }
    impl GhToken for SlowGh {
        fn token(&self) -> Result<String, SourceError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.lock().unwrap().send(()).unwrap();
                let _ = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(30));
            }
            Ok(TOKEN.to_string())
        }
    }
    let fake = FakeGitHub::start();
    fake.route("repos/o/n", Reply::Json(json!({"id": 1})));
    let (entered_tx, entered) = channel();
    let (release, release_rx) = channel();
    let slow = Arc::new(SlowGh {
        calls: AtomicUsize::new(0),
        entered: Mutex::new(entered_tx),
        release: Mutex::new(release_rx),
    });
    let gh = Arc::new(
        GitHub::new("gh", &fake.url(), slow.clone())
            .unwrap()
            .with_pause(|_| {}),
    );
    let (tx, rx) = channel();
    let ask = |gh: &Arc<GitHub>, tx: &Sender<bool>| {
        let (gh, tx) = (gh.clone(), tx.clone());
        std::thread::spawn(move || tx.send(gh.json("repos/o/n", 1).is_ok()).unwrap());
    };
    // 첫 스레드가 gh를 묻는 동안(들어온 걸 확인해요), 둘째 스레드는 세대를 읽고 gh_lease를 기다려요. 그때 연결을
    // 다시 고르고 첫 물음을 놓아 줘요. 둘째가 아직 기다리기 전이어도 결과는 같아요(새 세대로 물어요)
    ask(&gh, &tx);
    entered
        .recv_timeout(Duration::from_secs(5))
        .expect("gh was not asked");
    ask(&gh, &tx);
    std::thread::sleep(Duration::from_millis(50));
    gh.set_credentials("gh", None);
    release.send(()).unwrap();
    for _ in 0..2 {
        let ok = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a lease deadlocked");
        assert!(ok);
    }
    // 둘째 스레드는 새 세대로 gh에 다시 물었어요
    assert_eq!(slow.calls.load(Ordering::SeqCst), 2);
}

/// gh 토큰의 거절은 그 토큰에 붙어요: 401 뒤 gh가 같은 토큰을 주면 그대로 거절이고 묻지 않아요. 다른 토큰을 주면
/// `GET user`를 다시 물어요(`gh auth refresh`·재로그인).
#[test]
fn a_new_gh_token_is_judged_again() {
    let fake = FakeGitHub::start();
    fake.login(http(401, r#"{"message": "Bad credentials"}"#));
    fake.route("repos/o/n", http(401, r#"{"message": "Bad credentials"}"#));
    let gh_token = Arc::new(FakeGhToken::new(vec![
        Some("gho_old"),
        Some("gho_old"),
        Some(TOKEN),
    ]));
    let gh = gh_mode(&fake, &gh_token);
    assert!(!gh.connected_now()); // gho_old가 거절됐고 잊었어요
    assert_eq!((gh_token.calls(), fake.calls().len()), (1, 1));
    fake.login(Reply::Json(json!({"login": "basejb"})));
    // gh가 같은 토큰을 다시 줘요: 판단은 그대로이고 GET user를 묻지 않아요
    assert!(!gh.connected_now());
    assert_eq!((gh_token.calls(), fake.calls().len()), (2, 1));
    assert!(!gh.connected_now());
    assert_eq!(gh_token.calls(), 2); // 다시 빌린 토큰을 기억해요
    // 그 토큰이 다른 요청에서도 401이라 잊고, gh가 새 토큰을 줘요: 다시 판단해요
    assert_eq!(gh.json("repos/o/n", 1).unwrap_err().kind(), Kind::Key);
    assert!(gh.connected_now());
    assert_eq!(gh.login().as_deref(), Some("basejb"));
    assert_eq!(gh_token.calls(), 3);
    let users = fake.calls().iter().filter(|c| c.path == "user").count();
    assert_eq!(users, 2);
}

/// 여러 스레드가 같이 토큰이 필요해도 `gh auth token`은 한 번만 떠요.
#[test]
fn gh_is_asked_once_under_concurrency() {
    struct SlowGh(AtomicUsize);
    impl GhToken for SlowGh {
        fn token(&self) -> Result<String, SourceError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(300));
            Ok(TOKEN.to_string())
        }
    }
    let fake = FakeGitHub::start();
    fake.route("repos/o/n", Reply::Json(json!({"id": 1})));
    let slow = Arc::new(SlowGh(AtomicUsize::new(0)));
    let gh = Arc::new(
        GitHub::new("gh", &fake.url(), slow.clone())
            .unwrap()
            .with_pause(|_| {}),
    );
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let gh = gh.clone();
            std::thread::spawn(move || gh.json("repos/o/n", 1).unwrap())
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), json!({"id": 1}));
    }
    assert_eq!(slow.0.load(Ordering::SeqCst), 1);
    assert_eq!(fake.calls().len(), 2);
    assert!(fake.calls().iter().all(|c| c.has_token));
}

#[test]
fn login_refused_means_not_connected() {
    let fake = FakeGitHub::start();
    fake.login(Reply::Http {
        code: 401,
        headers: vec![],
        body: br#"{"message": "Bad credentials", "status": "401"}"#.to_vec(),
    });
    let gh = token(&fake);
    assert_eq!(gh.login_state(), LoginState::Refused);
    assert_eq!(gh.login(), None);
    assert!(gh.connected());
    assert!(!gh.connected_now());
    // 거절은 이 세대 동안 기억해요: 다시 물어도 요청이 늘지 않아요
    assert_eq!(fake.calls().len(), 1);
    // bad credentials의 403도 거절이에요
    fake.login(Reply::Http {
        code: 403,
        headers: vec![],
        body: br#"{"message": "Bad credentials"}"#.to_vec(),
    });
    gh.set_credentials("token", Some(TOKEN));
    assert_eq!(gh.login_state(), LoginState::Refused);
    assert_eq!(fake.calls().len(), 2);
    // 새 토큰은 다시 물어요
    fake.login(Reply::Json(json!({"login": "basejb"})));
    gh.set_credentials("token", Some(TOKEN));
    assert_eq!(gh.login_state(), LoginState::Known("basejb".into()));
    assert!(gh.connected_now());
    assert_eq!(gh.login().as_deref(), Some("basejb")); // 찾은 건 기억해요
    assert_eq!(fake.calls().len(), 3);
    for c in fake.calls() {
        assert_eq!((c.method.as_str(), c.path.as_str()), ("GET", "user"));
        assert!(c.has_token);
    }
}

#[test]
fn login_unknown_is_not_remembered() {
    let fake = FakeGitHub::start();
    let gh = token(&fake).with_timeouts(Timeouts {
        login: Duration::from_millis(200),
        ..Timeouts::default()
    });
    let unknown = [
        Reply::Close,
        Reply::Slow(
            Duration::from_secs(2),
            Box::new(Reply::Json(json!({"login": "x"}))),
        ),
        http(502, r#"{"message": "Server Error"}"#),
        Reply::Http {
            code: 403,
            headers: vec![("X-RateLimit-Remaining".into(), "0".into())],
            body: br#"{"message": "API rate limit exceeded"}"#.to_vec(),
        },
        http(429, "{}"),
        http(200, r#"{"id": 1}"#), // login이 없는 200
        http(200, "not json"),
    ];
    for (n, reply) in unknown.into_iter().enumerate() {
        fake.login(reply);
        assert_eq!(gh.login_state(), LoginState::Unknown, "#{n}");
        assert_eq!(gh.login(), None, "#{n}");
        // 오프라인은 연결이 없다는 뜻이 아니에요
        assert!(gh.connected_now(), "#{n}");
    }
    let before = fake.calls().len();
    assert_eq!(before, 7 * 3); // 모를 때는 기억하지 않아 부를 때마다 물어요
    fake.login(Reply::Json(json!({"login": "basejb"})));
    assert_eq!(gh.login().as_deref(), Some("basejb"));
    assert_eq!(gh.login().as_deref(), Some("basejb"));
    assert_eq!(fake.calls().len(), before + 1);
}

#[test]
fn gh_mode_asks_gh_once_and_again_after_401() {
    let fake = FakeGitHub::start();
    fake.route("repos/o/n", Reply::Json(json!({"id": 1})));
    let gh_token = Arc::new(FakeGhToken::gives(TOKEN));
    let gh = gh_mode(&fake, &gh_token);
    assert_eq!(gh_token.calls(), 0); // 만들 때는 묻지 않아요
    for _ in 0..3 {
        gh.json("repos/o/n", 1).unwrap();
    }
    assert_eq!(gh_token.calls(), 1); // 세대 동안 기억해요
    assert!(fake.calls().iter().all(|c| c.has_token));

    // 그 토큰이 401이면 잊고 다음 호출이 gh에 다시 물어요
    fake.route("repos/o/n", http(401, r#"{"message": "Bad credentials"}"#));
    let e = gh.json("repos/o/n", 1).unwrap_err();
    assert_eq!(
        (e.kind(), e.detail()),
        (
            Kind::Key,
            "GitHub answered HTTP 401 (bad credentials)".to_string()
        )
    );
    fake.route("repos/o/n", Reply::Json(json!({"id": 1})));
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 2);
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 2);
    // bad credentials의 403도 거절이에요(다른 403은 아니에요)
    fake.route(
        "repos/o/n",
        http(
            403,
            r#"{"message": "Resource not accessible by integration"}"#,
        ),
    );
    gh.json("repos/o/n", 1).unwrap_err();
    fake.route("repos/o/n", Reply::Json(json!({"id": 1})));
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 2);
    fake.route("repos/o/n", http(403, r#"{"message": "Bad credentials"}"#));
    gh.json("repos/o/n", 1).unwrap_err();
    fake.route("repos/o/n", Reply::Json(json!({"id": 1})));
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 3);
    // 목록과 GraphQL도 같아요
    fake.route("repos/o/n/branches", http(401, "{}"));
    assert_eq!(
        gh.pages("repos/o/n/branches").unwrap_err().kind(),
        Kind::Key
    );
    fake.graphql(|_, _| (401, json!({"message": "Bad credentials"})));
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 4);
    assert_eq!(
        gh.graphql("query{x}", "o", "n").unwrap_err().kind(),
        Kind::Key
    );
    gh.json("repos/o/n", 1).unwrap();
    // GraphQL은 3번 물어요: 실패마다 토큰을 잊고 다시 빌려요
    assert_eq!(gh_token.calls(), 4 + GRAPHQL_TRIES as usize);
    // 연결을 다시 고르면 다시 물어요
    gh.set_credentials("gh", None);
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 5 + GRAPHQL_TRIES as usize);
    // token 모드는 gh를 묻지 않아요
    gh.set_credentials("token", Some(TOKEN));
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 5 + GRAPHQL_TRIES as usize);
}

#[test]
fn github_mode_fails_closed() {
    let fake = FakeGitHub::start();
    let want = [
        ("gh", Mode::Gh),
        ("none", Mode::None),
        ("token", Mode::None),
        ("off", Mode::None),
        ("tacie", Mode::None),
        ("", Mode::None),
    ];
    for (value, mode) in want {
        assert_eq!(connect(value, &fake).mode(), mode, "{value:?}");
    }
    // test_github_mode_from_environment_fails_closed: 환경 변수의 값으로 시작해도 연결이 없어요
    for value in ["off", "token", "tacie", ""] {
        assert!(!connect(value, &fake).connected(), "{value:?}");
    }
    let gh = connect("none", &fake);
    for (mode, token) in [("token", Some("")), ("token", None), ("off", Some("gho_x"))] {
        gh.set_credentials(mode, token);
        assert!(
            !gh.connected() && gh.mode() == Mode::None,
            "{mode} {token:?}"
        );
        assert_eq!(gh.login(), None);
        assert!(is_key_error(gh.json("user", 1)));
    }
    assert!(fake.calls().is_empty());
}

#[test]
fn status_reads_any_answer() {
    let fake = FakeGitHub::start();
    fake.status(
        "h1",
        200,
        &[("X-RateLimit-Remaining", "4999")],
        Some(json!({"id": 7})),
    );
    fake.status("h2", 404, &[("X-GitHub-Request-Id", "AB:CD")], None);
    fake.status(
        "403",
        403,
        &[("x-ratelimit-remaining", "0")],
        Some(json!({"message": "API rate limit exceeded"})),
    );
    fake.route(
        "html",
        Reply::Http {
            code: 502,
            headers: vec![("Content-Type".into(), "text/html".into())],
            body: b"<html>bad</html>".to_vec(),
        },
    );
    fake.status_with(|path, etag| match etag {
        Some(e) => Reply::Http {
            code: 304,
            headers: vec![("ETag".into(), e.to_string())],
            body: vec![],
        },
        None => http(500, &format!("{{\"path\": \"{path}\"}}")),
    });
    let gh = token(&fake);
    let (status, headers, body) = gh.status("h1", &[], None).unwrap();
    assert_eq!((status, body), (200, Some(json!({"id": 7}))));
    assert_eq!(headers["x-ratelimit-remaining"], "4999");
    assert_eq!(headers["content-type"], "application/json");
    let (status, headers, body) = gh.status("h2", &[], None).unwrap();
    assert_eq!((status, body), (404, None));
    assert_eq!(headers["x-github-request-id"], "AB:CD");
    assert_eq!(
        gh.status("403", &[], None).unwrap().2,
        Some(json!({"message": "API rate limit exceeded"}))
    );
    let (status, headers, body) = gh.status("html", &[], None).unwrap();
    assert_eq!((status, body), (502, None)); // JSON이 아닌 몸통은 None이에요
    assert_eq!(headers["content-type"], "text/html");
    // 머리는 보낸 대로예요(ETag)
    let (status, headers, _) = gh
        .status(
            "other",
            &[("If-None-Match", "W/\"e0\"")],
            Some(CHECK_TIMEOUT),
        )
        .unwrap();
    assert_eq!((status, headers["etag"].as_str()), (304, "W/\"e0\""));
    let c = fake.calls().pop().unwrap();
    assert_eq!(c.if_none_match.as_deref(), Some("W/\"e0\""));
    assert_eq!(gh.status("other", &[], None).unwrap().0, 500);
    assert_eq!(fake.calls().len(), 6); // 상태가 무엇이든 한 번만 물어요
    let _ = names(&[]);
}

#[test]
fn gh_status_timeout_reads_as_network() {
    let fake = FakeGitHub::start();
    fake.route(
        "repos/o/n",
        Reply::Slow(Duration::from_secs(2), Box::new(Reply::Json(json!({})))),
    );
    let gh = token(&fake);
    let e = gh
        .status("repos/o/n", &[], Some(Duration::from_millis(200)))
        .unwrap_err();
    assert_eq!(e.kind(), Kind::Network, "{e:?}");
    assert!(e.detail().starts_with("GitHub could not be reached"));
    // 시간을 주지 않으면 요청 하나의 시간이에요
    let short = token(&fake).with_timeouts(Timeouts {
        request: Duration::from_millis(200),
        ..Timeouts::default()
    });
    assert_eq!(
        short.status("repos/o/n", &[], None).unwrap_err().kind(),
        Kind::Network
    );
    // 답 없이 끊겨도 network예요
    fake.route("repos/o/m", Reply::Close);
    assert_eq!(
        gh.status("repos/o/m", &[], None).unwrap_err().kind(),
        Kind::Network
    );
}

#[test]
fn gh_json_retries_cut_off_answers() {
    static PAUSES: Mutex<Vec<Duration>> = Mutex::new(Vec::new());
    fn record(d: Duration) {
        PAUSES.lock().unwrap().push(d);
    }
    let paused = || std::mem::take(&mut *PAUSES.lock().unwrap());
    let cut = || http(200, "[[{");
    let fake = FakeGitHub::start();
    let gh = token(&fake).with_pause(record);

    // "unexpected end of JSON input": 잘린 답 두 번 뒤 온전한 답
    fake.route_seq(
        "repos/o/n",
        vec![cut(), cut(), Reply::Json(json!({"id": 1}))],
    );
    assert_eq!(gh.json("repos/o/n", 3).unwrap(), json!({"id": 1}));
    assert_eq!(fake.calls().len(), 3);
    assert_eq!(paused(), [Duration::from_secs(2), Duration::from_secs(4)]);

    // 한 번만 묻는 것은 Json 실패예요
    fake.clear_calls();
    fake.route("repos/o/n", cut());
    let e = gh.json("repos/o/n", 1).unwrap_err();
    assert!(matches!(e, SourceError::Json(_)), "{e:?}");
    assert_eq!(e.kind(), Kind::Down);
    assert_eq!(fake.calls().len(), 1);
    assert!(paused().is_empty());

    // 실패한 답(500 두 번)도 다시 묻고, 끝까지 실패하면 마지막 실패의 종류예요
    fake.clear_calls();
    fake.route_seq(
        "x",
        vec![http(500, "{}"), http(500, "{}"), Reply::Json(json!([1]))],
    );
    assert_eq!(gh.json("x", 3).unwrap(), json!([1]));
    assert_eq!(fake.calls().len(), 3);
    assert_eq!(paused(), [Duration::from_secs(2), Duration::from_secs(4)]);
    fake.clear_calls();
    fake.route("y", http(502, r#"{"message": "secret-org"}"#));
    let e = gh.json("y", 3).unwrap_err();
    assert_eq!(
        (e.kind(), e.detail()),
        (Kind::Down, "GitHub answered HTTP 502".to_string())
    );
    assert_eq!(fake.calls().len(), 3);
    assert_eq!(paused(), [Duration::from_secs(2), Duration::from_secs(4)]);
    // 끊긴 연결도 다시 물어요
    fake.clear_calls();
    fake.route_seq("z", vec![Reply::Close, Reply::Json(json!(1))]);
    assert_eq!(gh.json("z", 3).unwrap(), json!(1));
    assert_eq!(paused(), [Duration::from_secs(2)]);

    // 토큰이 없으면(연결 없음) 다시 묻지 않아요
    gh.set_credentials("none", None);
    fake.clear_calls();
    assert!(is_key_error(gh.json("repos/o/n", 3)));
    assert!(fake.calls().is_empty());
    assert!(paused().is_empty());
}

#[test]
fn rest_asks_an_empty_list_again() {
    static PAUSES: Mutex<Vec<Duration>> = Mutex::new(Vec::new());
    fn record(d: Duration) {
        PAUSES.lock().unwrap().push(d);
    }
    let paused = || std::mem::take(&mut *PAUSES.lock().unwrap());
    let path = "repos/o/n/issues?state=all";
    let fake = FakeGitHub::start();
    let gh = token(&fake).with_pause(record);

    fake.route_seq(
        path,
        vec![
            Reply::Json(json!([])),
            Reply::Json(json!([])),
            Reply::Json(json!([{"id": 1}])),
        ],
    );
    assert_eq!(gh.rest(path, None, 3).unwrap(), vec![json!({"id": 1})]);
    assert_eq!(paused(), [Duration::from_secs(1), Duration::from_secs(2)]);
    let calls = fake.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(
        (calls[0].path.as_str(), calls[0].query.as_str()),
        ("repos/o/n/issues", "state=all&per_page=100")
    );

    // 끝까지 비면 빈 목록이 답이에요
    fake.clear_calls();
    fake.route(path, Reply::Json(json!([])));
    assert!(gh.rest(path, None, 3).unwrap().is_empty());
    assert_eq!(fake.calls().len(), 3);
    paused();

    // since 목록은 한 번만 물어요(새것 없음이 보통의 답이에요). 빈 since는 없는 것과 같아요
    fake.clear_calls();
    assert!(
        gh.rest(path, Some("2026-10-24T00:00:00Z"), 3)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        asked(&fake),
        [(
            "repos/o/n/issues".to_string(),
            "state=all&per_page=100&since=2026-10-24T00%3A00%3A00Z".to_string()
        )]
    );
    fake.clear_calls();
    gh.rest(path, Some(""), 2).unwrap();
    assert_eq!(
        asked(&fake),
        vec![
            (
                "repos/o/n/issues".to_string(),
                "state=all&per_page=100".to_string()
            );
            2
        ]
    );
    paused();

    // 안쪽은 잘린 답을 세 번까지 물어요
    fake.clear_calls();
    fake.route_seq(path, vec![http(200, "[{"), Reply::Json(json!([{"id": 2}]))]);
    assert_eq!(gh.rest(path, None, 3).unwrap(), vec![json!({"id": 2})]);
    assert_eq!(paused(), [Duration::from_secs(2)]);

    // 여러 쪽은 Link를 따라 이어요
    fake.clear_calls();
    let many: Vec<Value> = (0..230).map(|n| json!({"id": n})).collect();
    fake.route(path, Reply::Json(Value::Array(many.clone())));
    assert_eq!(gh.rest(path, None, 3).unwrap(), many);
    assert_eq!(fake.calls().len(), 3);
}

#[test]
fn repos_lists_and_flags_orgs_needing_approval() {
    let repos = "user/repos?affiliation=owner,collaborator,organization_member&sort=pushed";
    let fake = FakeGitHub::start();
    let mut listed_repos = vec![
        json!({"full_name": "o/old", "private": true, "default_branch": "main", "pushed_at": "2026-10-20T00:00:00Z", "x": 1}),
        json!({"full_name": "o/never", "private": null}),
    ];
    // 쪽을 넘겨도 같아요(가운데 저장소는 시각이 같아 받은 순서예요)
    for n in 0..100 {
        listed_repos.push(json!({"full_name": format!("o/filler{n}"), "private": false, "pushed_at": "2026-01-01T00:00:00Z"}));
    }
    listed_repos.push(json!({"full_name": "o/new", "private": false, "default_branch": "dev", "pushed_at": "2026-10-22T00:00:00Z"}));
    fake.route(repos, Reply::Json(Value::Array(listed_repos)));
    let orgs = [
        "example-corp",
        "sso-corp",
        "open-org",
        "b-corp",
        "c",
        "d-sso",
    ];
    let mut listed: Vec<Value> = orgs.iter().map(|o| json!({"login": o})).collect();
    listed.extend([
        json!({"login": "bad org"}),
        json!({"login": null}),
        json!({}),
        json!({"login": "x\n"}),
    ]);
    fake.route("user/orgs", Reply::Json(Value::Array(listed)));
    let oauth = |o: &str| {
        http(403, &json!({"message": format!(
            "Although you appear to have the correct authorization credentials, the `{o}` organization has enabled OAuth App access restrictions, meaning that data access to third-parties is limited."
        )}).to_string())
    };
    let sso = http(
        403,
        r#"{"message": "Resource protected by organization SAML enforcement. You must grant your Personal Access token access to this organization."}"#,
    );
    for o in orgs {
        let reply = match o {
            "example-corp" | "b-corp" => oauth(o),
            "sso-corp" | "d-sso" => sso.clone(),
            // 실패한 확인은 목록을 실패시키지 않아요
            "c" => Reply::Slow(Duration::from_secs(2), Box::new(Reply::Json(json!([])))),
            _ => Reply::Json(json!([])),
        };
        fake.route(&format!("orgs/{o}/repos?type=private"), reply);
    }
    let gh = token(&fake).with_timeouts(Timeouts {
        probe: Duration::from_millis(300),
        ..Timeouts::default()
    });
    let got = gh.repos().unwrap();
    assert_eq!(
        got.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["repos", "needs_approval", "needs_sso"]
    );
    let full: Vec<&str> = got["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["full_name"].as_str().unwrap())
        .collect();
    assert_eq!(full.len(), 103);
    // 최근에 푸시한 것부터, 시각이 없으면 뒤로
    assert_eq!(full[..3], ["o/new", "o/old", "o/filler0"]);
    assert_eq!(full[102], "o/never");
    let first = &got["repos"][0];
    assert_eq!(
        first.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["full_name", "private", "default_branch", "pushed_at"]
    );
    assert_eq!(
        *first,
        json!({"full_name": "o/new", "private": false, "default_branch": "dev", "pushed_at": "2026-10-22T00:00:00Z"})
    );
    assert_eq!(
        got["repos"][102],
        json!({"full_name": "o/never", "private": false, "default_branch": null, "pushed_at": null})
    );
    assert_eq!(got["needs_approval"], json!(["example-corp", "b-corp"]));
    assert_eq!(got["needs_sso"], json!(["sso-corp", "d-sso"]));
    // 조직 이름, GitHub의 글은 오류·응답 밖으로 나가지 않아요(안에서만 갈라요)
    let calls = fake.calls();
    let mut probed: Vec<&str> = calls
        .iter()
        .filter_map(|c| c.path.strip_prefix("orgs/"))
        .map(|p| p.split('/').next().unwrap())
        .collect();
    probed.sort();
    assert_eq!(
        probed,
        [
            "b-corp",
            "c",
            "d-sso",
            "example-corp",
            "open-org",
            "sso-corp"
        ]
    );
    for c in calls.iter().filter(|c| c.path.starts_with("orgs/")) {
        assert_eq!(c.query, "type=private&per_page=1");
        assert!(c.has_token);
    }
    // 두 목록은 한 번씩만 물어요(빈 목록도 진짜 답이에요). 저장소 목록은 두 쪽이에요
    let lists: Vec<String> = calls
        .iter()
        .filter(|c| !c.path.starts_with("orgs/"))
        .map(|c| c.path.clone())
        .collect();
    assert_eq!(
        lists,
        ["user/repos", "repositories/1/user/repos", "user/orgs"]
    );
}

#[test]
fn branches_puts_default_first() {
    let fake = FakeGitHub::start();
    fake.route(
        "repos/o/n",
        Reply::Json(json!({"default_branch": "main", "id": 1})),
    );
    let mut many: Vec<Value> = vec![json!({"name": "dev"}), json!({"name": "main"})];
    many.extend((0..99).map(|n| json!({"name": format!("b{n}")})));
    many.push(json!({"name": "feat"}));
    fake.route("repos/o/n/branches", Reply::Json(Value::Array(many)));
    let gh = token(&fake);
    let got = gh.branches("o/n").unwrap();
    let names: Vec<&str> = got["branches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 102);
    assert_eq!(names[..3], ["main", "dev", "b0"]);
    assert_eq!(names[101], "feat");
    assert_eq!(got["default"], "main");
    assert_eq!(
        got.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["branches", "default"]
    );
    let asked = asked(&fake);
    assert_eq!(asked[0], ("repos/o/n".to_string(), String::new()));
    assert_eq!(
        asked[1],
        ("repos/o/n/branches".to_string(), "per_page=100".to_string())
    );
    assert_eq!(asked.len(), 3); // 두 쪽
}

#[test]
fn valid_names() {
    for good in [
        "o/n",
        "acme/widgets",
        "0/n",
        "o/...",
        "o/.x",
        "o/a.b_c-d",
        "o-/n",
    ] {
        assert!(github::valid_repo_name(good), "{good:?}");
    }
    // %0A: `$`가 놓아줄 끝 줄바꿈이에요
    for bad in [
        "nope", "o/..", "o/.", "-x/y", "", "o/n\n", "o/n/x", "o/", "/n", "o/n m", "o_x/n", "o/한",
    ] {
        assert!(!github::valid_repo_name(bad), "{bad:?}");
    }
}

/// 표의 기대값은 진짜 `git check-ref-format`으로 한 번 만든 것이에요(`tests/fixtures/make_branch_names.sh`).
#[test]
fn branch_names_match_git_check_ref_format() {
    let table: Vec<(String, bool)> =
        serde_json::from_str(include_str!("fixtures/branch_names.json")).unwrap();
    assert!(table.len() >= 60, "{}", table.len());
    for (name, want) in table {
        assert_eq!(github::valid_branch(&name), want, "{name:?}");
    }
}

#[test]
fn rate_limited_reads_status_headers_and_message() {
    let none = HashMap::new();
    let left0 = names(&[("x-ratelimit-remaining", "0")]);
    let left9 = names(&[("x-ratelimit-remaining", "4999")]);
    assert!(github::rate_limited(429, &none, None));
    assert!(github::rate_limited(
        403,
        &left0,
        Some(&json!({"message": "x"}))
    ));
    assert!(github::rate_limited(
        403,
        &none,
        Some(&json!({"message": "API Rate Limit exceeded for user ID 1."}))
    ));
    assert!(github::rate_limited(
        403,
        &left9,
        Some(&json!({"message": "You have exceeded a secondary rate limit."}))
    ));
    assert!(!github::rate_limited(
        403,
        &left9,
        Some(&json!({"message": "Resource not accessible by integration"}))
    ));
    assert!(!github::rate_limited(
        403,
        &none,
        Some(&json!({"message": 1}))
    ));
    assert!(!github::rate_limited(
        403,
        &none,
        Some(&json!(["rate limit"]))
    ));
    assert!(!github::rate_limited(404, &left0, None));
    assert!(!github::rate_limited(
        401,
        &none,
        Some(&json!({"message": "rate limit"}))
    ));
}

#[test]
fn token_never_leaves_memory() {
    // 토큰은 Authorization 머리에만 있어요
    const SECRET: &str = FakeGitHub::TOKEN;
    let fake = FakeGitHub::start();
    fake.login(Reply::Json(json!({"login": "basejb"})));
    fake.route("repos/o/n", Reply::Json(json!({"default_branch": "main"})));
    fake.route("repos/o/gone", http(404, r#"{"message": "Not Found"}"#));
    fake.route(
        "orgs/o/repos",
        http(
            403,
            r#"{"message": "Resource protected by organization SAML enforcement."}"#,
        ),
    );
    fake.route("repos/o/offline", Reply::Close);
    fake.graphql(|_, _| (502, json!({"message": "x"})));
    let gh = connect("none", &fake);
    gh.set_credentials("token", Some(SECRET));

    assert_eq!(gh.login().as_deref(), Some("basejb"));
    gh.json("repos/o/n", 1).unwrap();
    let errors = [
        gh.json("repos/o/gone", 2).unwrap_err(),
        gh.status("repos/o/offline", &[], Some(CHECK_TIMEOUT))
            .unwrap_err(),
        gh.graphql("query{x}", "o", "n").unwrap_err(),
        gh.pages("repos/o/gone").unwrap_err(),
    ];
    let probe = gh.probe("orgs/o/repos?type=private&per_page=1");
    assert!(!probe.0);

    // 가짜 서버의 기록에는 토큰 값이 없고, 맞는 토큰이었는지만 있어요
    let calls = fake.calls();
    assert!(calls.len() >= 6);
    for c in &calls {
        assert!(c.has_token, "{c:?}");
        let text = format!("{c:?}");
        assert!(!text.contains(SECRET), "{text}");
    }
    for e in &errors {
        for text in [e.detail(), e.to_string(), format!("{e:?}")] {
            assert!(!text.contains(SECRET), "error: {text}");
            assert!(!text.contains("127.0.0.1"), "error: {text}");
        }
    }
    assert!(!probe.1.contains(SECRET));
    let debug = format!("{gh:?}");
    assert!(!debug.contains(SECRET), "{debug}");
}

// ---- REST 클라이언트(`github::http`)와 가짜 GitHub 서버(`testing::github`) ----

mod rest {
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};
    use tacie_sources::error::{Kind, SourceError};
    use tacie_sources::github::http::{Accept, Http, Target, segment, with_query};
    use tacie_sources::testing::github::{Call, FakeGitHub, Reply};

    const T: Duration = Duration::from_secs(5);
    const TOKEN: &str = FakeGitHub::TOKEN;

    fn http(fake: &FakeGitHub) -> Http {
        Http::new(&fake.url()).unwrap()
    }

    fn gets(calls: &[Call]) -> Vec<(String, String)> {
        calls
            .iter()
            .map(|c| (c.path.clone(), c.query.clone()))
            .collect()
    }

    #[test]
    fn requests_send_user_agent_version_and_token() {
        let fake = FakeGitHub::start();
        fake.login(Reply::Json(json!({"login": "octo"})));
        let h = http(&fake);
        let a = h
            .get(TOKEN, Target::Path("user"), Accept::Json, &[], T)
            .unwrap();
        assert_eq!(a.status, 200);
        assert_eq!(
            serde_json::from_slice::<Value>(&a.body).unwrap()["login"],
            "octo"
        );
        let calls = fake.calls();
        assert_eq!(calls.len(), 1);
        let c = &calls[0];
        assert_eq!((c.method.as_str(), c.path.as_str()), ("GET", "user"));
        assert!(c.has_token);
        assert!(c.user_agent.as_deref().unwrap().starts_with("Tacie/"));
        assert_eq!(c.accept.as_deref(), Some("*/*"));
        assert_eq!(c.api_version.as_deref(), Some("2022-11-28"));
        // 다른 토큰은 맞지 않아요
        h.get("ghp_other", Target::Path("user"), Accept::Raw, &[], T)
            .unwrap();
        let c = fake.calls().pop().unwrap();
        assert!(!c.has_token);
        assert_eq!(c.accept.as_deref(), Some("application/vnd.github.raw+json"));
        assert_eq!(h.base(), fake.url());
    }

    #[test]
    fn pages_follow_link_next() {
        let fake = FakeGitHub::start();
        let items: Vec<Value> = (0..250).map(|n| json!({"n": n})).collect();
        fake.route(
            "repos/o/r/issues?state=all",
            Reply::Json(Value::Array(items.clone())),
        );
        let h = http(&fake);
        let got = h
            .pages(
                TOKEN,
                "repos/o/r/issues?state=all",
                &[("since", "2026-10-26T01:02:03Z")],
                T,
            )
            .unwrap();
        assert_eq!(got, items);
        let calls = gets(&fake.calls());
        assert_eq!(calls.len(), 3);
        assert_eq!(
            calls[0],
            (
                "repos/o/r/issues".to_string(),
                "state=all&per_page=100&since=2026-10-26T01%3A02%3A03Z".to_string()
            )
        );
        for (n, (path, query)) in calls[1..].iter().enumerate() {
            let page = n + 2;
            assert_eq!(path, "repositories/1/issues");
            assert_eq!(query.matches("per_page=100").count(), 1, "{query}");
            assert!(
                query.ends_with(&format!("&after=c{page}&page={page}")),
                "{query}"
            );
        }
    }

    #[test]
    fn link_to_another_origin_is_refused() {
        let other = FakeGitHub::start();
        let fake = FakeGitHub::start();
        let link = format!("<{}repos/o/r/issues?page=2>; rel=\"next\"", other.url());
        fake.route(
            "repos/o/r/issues",
            Reply::Http {
                code: 200,
                headers: vec![("Link".into(), link)],
                body: b"[1]".to_vec(),
            },
        );
        let err = http(&fake)
            .pages(TOKEN, "repos/o/r/issues", &[], T)
            .unwrap_err();
        assert_eq!(err.kind(), Kind::Down, "{err:?}");
        assert!(!err.detail().contains("127.0.0.1"), "{}", err.detail());
        assert!(other.calls().is_empty());
        assert_eq!(fake.calls().len(), 1);
    }

    #[test]
    fn a_repeated_next_link_is_down() {
        let fake = FakeGitHub::start();
        let link = format!("<{}repos/o/r/issues?page=2>; rel=\"next\"", fake.url());
        fake.route(
            "repos/o/r/issues",
            Reply::Http {
                code: 200,
                headers: vec![("Link".into(), link)],
                body: b"[1]".to_vec(),
            },
        );
        let err = http(&fake)
            .pages(TOKEN, "repos/o/r/issues", &[], T)
            .unwrap_err();
        assert_eq!(err.kind(), Kind::Down, "{err:?}");
        assert!(!err.detail().contains("127.0.0.1"), "{}", err.detail());
        // 첫 쪽, 둘째 쪽(처음 본 주소), 그리고 같은 주소를 다시 받은 데서 멈춰요
        assert_eq!(fake.calls().len(), 2);
    }

    #[test]
    fn a_relative_next_link_is_joined_and_a_broken_one_is_down() {
        let fake = FakeGitHub::start();
        fake.route(
            "repos/o/r/issues",
            Reply::Http {
                code: 200,
                headers: vec![("Link".into(), "</other/list?page=2>; rel=\"next\"".into())],
                body: b"[1]".to_vec(),
            },
        );
        fake.route("other/list", Reply::Json(json!([2])));
        let got = http(&fake)
            .pages(TOKEN, "repos/o/r/issues", &[], T)
            .unwrap();
        assert_eq!(got, vec![json!(1), json!(2)]);
        assert_eq!(fake.calls()[1].path, "other/list");

        fake.route(
            "repos/o/r/pulls",
            Reply::Http {
                code: 200,
                headers: vec![("Link".into(), "<http://[broken>; rel=\"next\"".into())],
                body: b"[1]".to_vec(),
            },
        );
        let err = http(&fake)
            .pages(TOKEN, "repos/o/r/pulls", &[], T)
            .unwrap_err();
        assert_eq!(err.kind(), Kind::Down, "{err:?}");
        assert!(!err.detail().contains("broken"), "{}", err.detail());
    }

    #[test]
    fn a_failed_redirect_leaves_no_token_in_the_error() {
        let fake = FakeGitHub::start();
        fake.route(
            "repos/o/r/tarball/abc",
            Reply::Http {
                code: 303,
                headers: vec![(
                    "Location".into(),
                    "http://127.0.0.1:9/codeload/o/r?token=SECRET".into(),
                )],
                body: vec![],
            },
        );
        let err = http(&fake)
            .get(
                TOKEN,
                Target::Path("repos/o/r/tarball/abc"),
                Accept::Tarball,
                &[],
                T,
            )
            .unwrap_err();
        assert_eq!(err.kind(), Kind::Network, "{err:?}");
        for text in [err.detail(), format!("{err:?}")] {
            assert!(!text.contains("SECRET"), "{text}");
            assert!(!text.contains("codeload"), "{text}");
        }
    }

    #[test]
    fn query_values_are_encoded() {
        assert_eq!(
            with_query(
                "repos/o/r/issues?state=all",
                &[("since", "2026-10-26T01:02:03Z")]
            ),
            "repos/o/r/issues?state=all&since=2026-10-26T01%3A02%3A03Z"
        );
        assert_eq!(
            with_query("user/repos", &[("per_page", "100"), ("q", "a b&c")]),
            "user/repos?per_page=100&q=a+b%26c"
        );
        assert_eq!(with_query("user", &[]), "user");
    }

    #[test]
    fn branch_segments_are_encoded() {
        assert_eq!(segment("a#b"), "a%23b");
        assert_eq!(segment("main"), "main");
        assert_eq!(segment("v1.2-rc_3~x"), "v1.2-rc_3~x");
        assert_eq!(segment("한"), "%ED%95%9C");
        assert_eq!(segment("a/b"), "a%2Fb");
        assert_eq!(segment("a?b c%"), "a%3Fb%20c%25");
        let fake = FakeGitHub::start();
        let path = format!("repos/o/r/git/ref/heads/feat/{}", segment("a#b"));
        fake.route(&path, Reply::Json(json!({"object": {"sha": "abc"}})));
        let a = http(&fake)
            .get(TOKEN, Target::Path(&path), Accept::Json, &[], T)
            .unwrap();
        assert_eq!(a.status, 200);
        assert_eq!(fake.calls()[0].path, "repos/o/r/git/ref/heads/feat/a%23b");
    }

    #[test]
    fn errors_carry_no_url_or_token() {
        let fake = FakeGitHub::start();
        let secret_path = "repos/secret-org/secret-repo/issues";
        let leaks = |e: &SourceError| {
            let d = e.detail();
            let dbg = format!("{e:?}");
            for text in [&d, &dbg] {
                assert!(!text.contains("127.0.0.1"), "{text}");
                assert!(!text.contains("secret"), "{text}");
                assert!(!text.contains(TOKEN), "{text}");
            }
            d
        };
        let h = http(&fake);
        for (code, body, kind) in [
            (
                401,
                json!({"message": "Bad credentials secret-org"}),
                Kind::Key,
            ),
            (
                403,
                json!({"message": "Forbidden for secret-org"}),
                Kind::Key,
            ),
            (404, json!({"message": "Not Found secret-repo"}), Kind::Key),
            (500, json!({"message": "oops secret"}), Kind::Down),
        ] {
            fake.status(secret_path, code, &[], Some(body));
            let e = h.pages(TOKEN, secret_path, &[], T).unwrap_err();
            assert_eq!(e.kind(), kind, "{code}");
            let d = leaks(&e);
            assert!(
                d.starts_with(&format!("GitHub answered HTTP {code}")),
                "{d}"
            );
        }
        // 연결 끊김
        fake.route(secret_path, Reply::Close);
        let e = h.pages(TOKEN, secret_path, &[], T).unwrap_err();
        assert_eq!(e.kind(), Kind::Network, "{e:?}");
        assert!(leaks(&e).starts_with("GitHub could not be reached"));
        // 시간 초과
        fake.route(
            secret_path,
            Reply::Slow(Duration::from_secs(2), Box::new(Reply::Json(json!([])))),
        );
        let e = h
            .get(
                TOKEN,
                Target::Path(secret_path),
                Accept::Json,
                &[],
                Duration::from_millis(200),
            )
            .unwrap_err();
        assert_eq!(e.kind(), Kind::Network, "{e:?}");
        assert!(leaks(&e).starts_with("GitHub could not be reached"));
        // 아무도 듣지 않는 곳
        let closed = Http::new("http://127.0.0.1:9/").unwrap();
        let e = closed
            .get(TOKEN, Target::Path(secret_path), Accept::Json, &[], T)
            .unwrap_err();
        assert_eq!(e.kind(), Kind::Network, "{e:?}");
        leaks(&e);
        // 머리에 실을 수 없는 토큰
        let e = h
            .get("bad\ntoken", Target::Path("user"), Accept::Json, &[], T)
            .unwrap_err();
        assert_eq!(e.kind(), Kind::Key, "{e:?}");
        assert!(!format!("{e:?}").contains("bad\ntoken"));
    }

    #[test]
    fn a_slow_answer_reads_as_network() {
        let fake = FakeGitHub::start();
        fake.route(
            "user",
            Reply::Slow(
                Duration::from_secs(2),
                Box::new(Reply::Json(json!({"login": "x"}))),
            ),
        );
        let start = Instant::now();
        let e = http(&fake)
            .get(
                TOKEN,
                Target::Path("user"),
                Accept::Json,
                &[],
                Duration::from_millis(300),
            )
            .unwrap_err();
        assert_eq!(e.kind(), Kind::Network, "{e:?}");
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn post_json_reaches_graphql() {
        let fake = FakeGitHub::start();
        fake.graphql(|query, vars| (200, json!({"data": {"q": query, "v": vars}})));
        let body = json!({"query": "query{x}", "variables": {"owner": "o", "name": "r"}});
        let a = http(&fake).post_json(TOKEN, "graphql", &body, T).unwrap();
        assert_eq!(a.status, 200);
        let v: Value = serde_json::from_slice(&a.body).unwrap();
        assert_eq!(v["data"]["q"], "query{x}");
        assert_eq!(v["data"]["v"], json!({"owner": "o", "name": "r"}));
        let c = &fake.calls()[0];
        assert_eq!(
            (c.method.as_str(), c.path.as_str(), c.has_token),
            ("POST", "graphql", true)
        );
    }

    #[test]
    fn fake_github_answers_as_documented() {
        let fake = FakeGitHub::start();
        let h = http(&fake);
        let get = |path: &str, headers: &[(&str, &str)]| {
            h.get(TOKEN, Target::Path(path), Accept::Json, headers, T)
                .unwrap()
        };
        let json_of = |a: &tacie_sources::github::http::Answer| -> Value {
            serde_json::from_slice(&a.body).unwrap()
        };
        assert!(fake.url().starts_with("http://127.0.0.1:") && fake.url().ends_with('/'));

        // route: 늘 같은 답, 목록은 per_page가 없으면 통째로
        fake.route("repos/o/r", Reply::Json(json!({"id": 7})));
        assert_eq!(json_of(&get("repos/o/r", &[])), json!({"id": 7}));
        assert_eq!(json_of(&get("repos/o/r", &[])), json!({"id": 7}));
        fake.route("repos/o/r/pulls", Reply::Json(json!([1, 2, 3])));
        let a = get("repos/o/r/pulls?per_page=2", &[]);
        assert_eq!(json_of(&a), json!([1, 2]));
        let next = a.next.clone().unwrap();
        assert_eq!(
            next.as_str(),
            format!(
                "{}repositories/1/pulls?per_page=2&after=c2&page=2",
                fake.url()
            )
        );
        let b = h
            .get(TOKEN, Target::Next(&next), Accept::Json, &[], T)
            .unwrap();
        assert_eq!(json_of(&b), json!([3]));
        assert!(b.next.is_none());
        assert_eq!(json_of(&get("repos/o/r/pulls", &[])), json!([1, 2, 3]));

        // 쿼리가 있는 길은 그 값이 모두 있는 요청에 답하고, 더 자세한 길이 이겨요
        fake.route("repos/o/r/issues", Reply::Json(json!(["any"])));
        fake.route("repos/o/r/issues?state=all", Reply::Json(json!(["all"])));
        assert_eq!(
            json_of(&get("repos/o/r/issues?state=all&since=x", &[])),
            json!(["all"])
        );
        assert_eq!(
            json_of(&get("repos/o/r/issues?state=open", &[])),
            json!(["any"])
        );

        // route_seq: 차례로, 마지막을 되풀이
        fake.route_seq(
            "repos/o/r/labels",
            vec![Reply::Json(json!([])), Reply::Json(json!(["x"]))],
        );
        for want in [json!([]), json!(["x"]), json!(["x"])] {
            assert_eq!(json_of(&get("repos/o/r/labels", &[])), want);
        }

        // status: 상태·머리·몸통 그대로, status_seq
        fake.status(
            "repos/o/r/git/ref/heads/main",
            404,
            &[("X-Ratelimit-Remaining", "9")],
            None,
        );
        let a = get("repos/o/r/git/ref/heads/main", &[]);
        assert_eq!(a.status, 404);
        assert_eq!(a.headers["x-ratelimit-remaining"], "9");
        assert!(a.body.is_empty());
        fake.status_seq(
            "repos/o/r/git/ref/heads/dev",
            vec![
                Reply::Http {
                    code: 500,
                    headers: vec![],
                    body: vec![],
                },
                Reply::Json(json!({"ok": 1})),
            ],
        );
        assert_eq!(get("repos/o/r/git/ref/heads/dev", &[]).status, 500);
        assert_eq!(get("repos/o/r/git/ref/heads/dev", &[]).status, 200);

        // status_with: 표에 없는 GET 모두, If-None-Match를 받아요 (ETag 다시 쓰기)
        fake.status_with(|path, etag| match (path, etag) {
            ("repos/o/r/commits?per_page=1", Some("\"e1\"")) => Reply::Http {
                code: 304,
                headers: vec![("ETag".into(), "\"e1\"".into())],
                body: vec![],
            },
            ("repos/o/r/commits?per_page=1", _) => Reply::Http {
                code: 200,
                headers: vec![("ETag".into(), "\"e1\"".into())],
                body: b"[]".to_vec(),
            },
            _ => Reply::Http {
                code: 404,
                headers: vec![],
                body: vec![],
            },
        });
        let first = get("repos/o/r/commits?per_page=1", &[]);
        assert_eq!(first.status, 200);
        let etag = first.headers["etag"].clone();
        let again = get("repos/o/r/commits?per_page=1", &[("If-None-Match", &etag)]);
        assert_eq!(again.status, 304);
        assert_eq!(
            fake.calls().last().unwrap().if_none_match.as_deref(),
            Some("\"e1\"")
        );
        assert_eq!(get("nowhere", &[]).status, 404);

        // login
        fake.login(Reply::Http {
            code: 401,
            headers: vec![],
            body: br#"{"message":"Bad credentials"}"#.to_vec(),
        });
        assert_eq!(get("user", &[]).status, 401);

        // calls, clear_calls
        assert!(!fake.calls().is_empty());
        fake.clear_calls();
        assert!(fake.calls().is_empty());

        // 답을 정하지 않은 요청: 500 + unanswered(읽으면 비워요)
        let lonely = FakeGitHub::start();
        let h2 = Http::new(&lonely.url()).unwrap();
        let a = h2
            .get(
                TOKEN,
                Target::Path("repos/x/y?state=all"),
                Accept::Json,
                &[],
                T,
            )
            .unwrap();
        assert_eq!(a.status, 500);
        let b = h2.post_json(TOKEN, "graphql", &json!({}), T).unwrap();
        assert_eq!(b.status, 500);
        assert_eq!(
            lonely.unanswered(),
            [
                "GET repos/x/y?state=all".to_string(),
                "POST graphql".to_string()
            ]
        );
        assert!(lonely.unanswered().is_empty());
    }

    #[test]
    fn fake_github_panics_on_drop_with_unanswered_requests() {
        let result = std::panic::catch_unwind(|| {
            let fake = FakeGitHub::start();
            let h = Http::new(&fake.url()).unwrap();
            h.get(TOKEN, Target::Path("nobody"), Accept::Json, &[], T)
                .unwrap();
            drop(fake);
        });
        assert!(result.is_err());
    }
}

#[test]
fn list_judges_bad_credentials_403_like_get() {
    let fake = FakeGitHub::start();
    fake.route("repos/o/n", Reply::Json(json!({"id": 1})));
    let gh_token = Arc::new(FakeGhToken::gives(TOKEN));
    let gh = gh_mode(&fake, &gh_token);
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 1);
    // 다른 403은 거절이 아니에요: 토큰을 잊지 않아요
    fake.route(
        "repos/o/n/branches",
        http(
            403,
            r#"{"message": "Resource not accessible by integration"}"#,
        ),
    );
    assert_eq!(
        gh.pages("repos/o/n/branches").unwrap_err().kind(),
        Kind::Key
    );
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 1);
    // bad credentials의 403은 거절이에요: 다음 호출이 gh에 다시 물어요
    fake.route(
        "repos/o/n/branches",
        http(403, r#"{"message": "Bad credentials"}"#),
    );
    assert_eq!(
        gh.pages("repos/o/n/branches").unwrap_err().kind(),
        Kind::Key
    );
    gh.json("repos/o/n", 1).unwrap();
    assert_eq!(gh_token.calls(), 2);
}
