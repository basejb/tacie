//! The GitHub connection: how to reach it (mode and token), GitHub REST calls (`http`) and
//! reading their answers, login checks, and repo and branch lists. Repo content (head, tree, blobs, tarball) is
//! `content`, issue and PR raw files and state are `items`,
//! documents and chunks are `docs`, extra PR info (GraphQL) is `extras`, repo checks and change checks (ETag) are
//! `probe`, refetching whole documents is `refetch`, weekly reconcile is `reconcile`, and cursor-based incremental
//! sync is `fetch`.
//!
//! The token lives only inside this struct. It goes to GitHub only in the `Authorization` header and never shows in
//! URLs, errors, logs or `Debug` output. When the connection is `none` it asks nothing and returns a key error. gh
//! mode borrows the token once with `gh auth token` the first time it is needed (`gh_token`), keeps it for that
//! generation, and forgets it on a 401.

pub mod branch;
pub mod content;
pub mod docs;
pub mod extras;
pub mod fetch;
pub mod gh_token;
pub mod http;
pub mod items;
pub mod probe;
pub mod reconcile;
pub mod refetch;

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Map, Value, json};

use crate::error::{Kind, SourceError};
use crate::py;
use gh_token::{GhToken, NOT_LOGGED_IN};
use http::{Accept, Answer, Http, REQUEST_TIMEOUT, Target, checked, kind_of};

pub use branch::valid_branch;

/// `GET user`(로그인 확인)를 기다리는 시간이에요.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(5);
/// 조직 하나를 확인하는 시간이에요. 4개씩 도니까 목록이 앱의 40 s 안에 와요.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// 확인(check)에서 GitHub 호출 하나를 기다리는 시간이에요.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
/// GitHub의 시각 글자예요. 커서는 글자로 비교해요(시간 순서와 같아요).
pub const ISO: &str = "%Y-%m-%dT%H:%M:%SZ";
/// 조직 확인을 한 번에 몇 개씩 하는지예요.
pub const PROBE_WORKERS: usize = 4;
/// gh가 토큰을 주지 못하면(gh 없음, 로그아웃, 느림) 이만큼 지나서 다시 물어요.
pub const GH_RETRY: Duration = Duration::from_secs(60);
/// GraphQL 질의 하나를 몇 번까지 묻는지예요.
pub const GRAPHQL_TRIES: u32 = 3;

const NOT_CONNECTED: &str = "GitHub is not connected";
const REPOS: &str = "user/repos?affiliation=owner,collaborator,organization_member&sort=pushed";

/// 지금 UTC 시각의 `ISO` 글자예요.
pub fn now_iso() -> String {
    chrono::Utc::now().format(ISO).to_string()
}

/// 저장소 이름 모양이에요. 이름이 `.`·`..`가 아닌지(`(?!\.{1,2}$)`)는 `valid_repo_name`이 따로 봐요.
static REPO_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9._-]+$").unwrap());
static ORG_LOGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9-]+$").unwrap());

/// `status`의 답이에요: (HTTP 상태, 소문자 이름의 머리, JSON 몸통 또는 None).
pub type Status = (u16, HashMap<String, String>, Option<Value>);

/// GitHub에 닿는 길이에요.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 사용자의 `gh` 로그인이에요(`gh auth token`으로 빌린 토큰).
    Gh,
    /// Tacie가 받은 토큰이에요.
    Token,
    /// 연결이 없어요.
    None,
}

/// 모드를 정리해요: "token"(토큰과 함께), "gh", "none"만 되고 나머지는 모두 닫힌 쪽(None)이에요.
fn norm(mode: &str, token: Option<&str>) -> (Mode, Option<String>) {
    match (mode, token) {
        ("gh", _) => (Mode::Gh, None),
        ("token", Some(t)) if !t.is_empty() => (Mode::Token, Some(t.to_string())),
        _ => (Mode::None, None),
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `GET user`로 본 로그인이에요(스펙 6-2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginState {
    /// 이 계정이에요. 그 연결 세대 동안 기억해요.
    Known(String),
    /// 토큰이 거절됐어요(401, `bad credentials`의 403, gh 모드에서 `gh auth token` 실패). 그 세대 동안 기억해요.
    Refused,
    /// 알 수 없어요(연결이 없음, 네트워크 실패, 시간 초과, 5xx, 호출 제한). 기억하지 않아요.
    Unknown,
}

impl LoginState {
    /// `Known`일 때만 그 이름이에요.
    pub fn name(&self) -> Option<String> {
        match self {
            LoginState::Known(name) => Some(name.clone()),
            _ => None,
        }
    }
}

/// 요청의 시간 제한이에요. 테스트가 짧게 바꿔요(`with_timeouts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// `GET user`예요.
    pub login: Duration,
    /// 조직 확인(`probe`) 하나예요.
    pub probe: Duration,
    /// 확인(check)의 물음 하나예요(바뀜 확인, 저장소가 아직 있는지).
    pub check: Duration,
    /// 목록의 쪽 하나, 문서, GraphQL, 시간을 주지 않은 `status` 요청 하나예요.
    pub request: Duration,
    /// gh 모드에서 gh가 토큰을 주지 못한 뒤 다시 묻기까지예요(그동안은 묻지 않고 거절이에요).
    pub gh_retry: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            login: LOGIN_TIMEOUT,
            probe: PROBE_TIMEOUT,
            check: CHECK_TIMEOUT,
            request: REQUEST_TIMEOUT,
            gh_retry: GH_RETRY,
        }
    }
}

/// 연결 세대마다 기억하는 것이에요. 연결을 바꾸면 세대가 올라가 앞 연결의 답은 기억하지 않아요.
#[derive(Default)]
struct Memory {
    generation: u64,
    /// `Known`이나 `Refused`예요(`Unknown`은 넣지 않아요: 로그인 때 켜진 앱은 Wi-Fi보다 먼저 돌아요).
    login: Option<LoginState>,
    /// `login`을 판단한 토큰이에요(gh가 토큰을 주지 못한 거절은 None). gh 모드에서 gh가 다른 토큰을 빌려 주면
    /// `login`을 잊어요(`gh auth refresh`·재로그인을 따라가요).
    judged: Option<String>,
    /// gh 모드에서 빌린 토큰이에요. 401(또는 bad credentials의 403)을 받으면 잊어요.
    gh_token: Option<String>,
    /// gh가 토큰을 주지 못한 때부터 이 시각까지는 다시 묻지 않아요.
    gh_quiet_until: Option<Instant>,
}

/// 요청 하나에 쓰는 토큰과, 그것을 얻은 세대예요.
struct Lease {
    token: String,
    generation: u64,
    from_gh: bool,
}

/// GitHub 연결 하나예요. 엔진에 하나 두고 갱신·확인·HTTP 경로가 같이 써요.
pub struct GitHub {
    /// 모드와 토큰을 한 덩어리로 바꾸고 한 번에 읽어요: 다른 스레드가 토큰 없는 모드를 보는 일이 없어요.
    cred: RwLock<(Mode, Option<String>)>,
    /// 로그인과 gh 토큰의 기억이에요. 잠그는 순서는 이것 다음 `cred`예요(`set_credentials`와 같아요).
    memory: Mutex<Memory>,
    /// gh에 토큰을 묻는 동안 쥐어요: 여러 스레드가 같이 필요해도 `gh auth token`은 한 번만 떠요.
    gh_lease: Mutex<()>,
    /// 지금 시각이에요(gh 실패 뒤 기다리기). 테스트는 시계를 넘겨요(`with_clock`).
    clock: Arc<dyn Fn() -> Instant + Send + Sync>,
    http: Http,
    gh_token: Arc<dyn GhToken>,
    /// gh 모드를 쓸 수 있는지예요: 시작할 때의 모드가 gh일 때만(개발 앱이 `TACIE_GITHUB_MODE=gh`로 띄울 때만)
    /// 참이에요. 아니면 `set_credentials("gh")`도 none이라 `gh`를 띄우지 않아요.
    gh_allowed: bool,
    timeouts: Timeouts,
    /// 다시 묻기 전에 쉬어요. 테스트는 쉬지 않게 바꿔요.
    pause: fn(Duration),
}

impl fmt::Debug for GitHub {
    /// 모드와 주소만 보여요. 토큰은 넣지 않아요.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHub")
            .field("mode", &self.mode())
            .field("api", &self.http.base())
            .finish_non_exhaustive()
    }
}

impl GitHub {
    /// `mode`는 `TACIE_GITHUB_MODE`의 값이에요(없으면 엔진이 "none"을 줘요). 토큰 없이 시작하니 "token"도 연결이
    /// 없어요. 시작 모드가 gh일 때만 나중에 gh 모드로 바꿀 수 있어요(`gh_allowed`).
    /// `api`는 REST 기본 주소(`http::API`, 테스트는 가짜 서버)예요. 주소를 읽을 수 없으면 down이에요. 아무것도
    /// 묻지 않고 프로세스도 띄우지 않아요(gh 토큰은 처음 필요할 때 빌려요).
    pub fn new(mode: &str, api: &str, gh_token: Arc<dyn GhToken>) -> Result<GitHub, SourceError> {
        let cred = norm(mode, None);
        Ok(GitHub {
            gh_allowed: cred.0 == Mode::Gh,
            cred: RwLock::new(cred),
            memory: Mutex::new(Memory::default()),
            gh_lease: Mutex::new(()),
            clock: Arc::new(Instant::now),
            http: Http::new(api)?,
            gh_token,
            timeouts: Timeouts::default(),
            pause: std::thread::sleep,
        })
    }

    /// 다시 묻기 전 쉬기예요(기본 `thread::sleep`).
    pub fn with_pause(mut self, pause: fn(Duration)) -> Self {
        self.pause = pause;
        self
    }

    /// 시각을 주는 함수예요(기본 `Instant::now`).
    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> Instant + Send + Sync>) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_timeouts(mut self, t: Timeouts) -> Self {
        self.timeouts = t;
        self
    }

    /// 어떻게 닿을지 고르고 기억(로그인, 빌린 gh 토큰)을 잊어요. 모드와 토큰은 한 번에 바뀌어요.
    /// gh 모드를 쓸 수 없는 엔진(`gh_allowed`가 거짓)에서 gh는 none이에요.
    pub fn set_credentials(&self, mode: &str, token: Option<&str>) {
        let mut memory = lock(&self.memory);
        let cred = match norm(mode, token) {
            (Mode::Gh, _) if !self.gh_allowed => (Mode::None, None),
            cred => cred,
        };
        *self.cred.write().unwrap_or_else(PoisonError::into_inner) = cred;
        memory.generation += 1;
        memory.login = None;
        memory.judged = None;
        memory.gh_token = None;
        memory.gh_quiet_until = None;
    }

    /// 연결의 세대예요. `set_credentials`마다 올라가요: 갱신은 시작할 때의 세대가 바뀌면(연결 해제, 다른 연결) 멈춰요.
    pub fn generation(&self) -> u64 {
        lock(&self.memory).generation
    }

    /// gh 모드를 쓸 수 있는 엔진인지예요(시작 모드가 gh).
    pub fn gh_allowed(&self) -> bool {
        self.gh_allowed
    }

    /// 지금의 시간 제한이에요.
    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    fn cred(&self) -> (Mode, Option<String>) {
        self.cred
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn mode(&self) -> Mode {
        self.cred.read().unwrap_or_else(PoisonError::into_inner).0
    }

    /// 모드가 none이 아니에요. 갱신·확인이 "연결이 없으면 아무것도 묻지 않음"으로 써요(거절된 토큰도 참이에요).
    pub fn connected(&self) -> bool {
        self.mode() != Mode::None
    }

    /// 응답에 보이는 연결이에요(스펙 6-2): 모드가 none이 아니고 토큰이 거절되지 않았어요.
    pub fn connected_now(&self) -> bool {
        self.connected() && self.login_state() != LoginState::Refused
    }

    /// 이 요청의 토큰이에요. token 모드는 그 토큰, gh 모드는 이 세대에 빌린 토큰(없으면 gh에 물어요), none은 key예요.
    /// gh가 토큰을 주지 못하면 key `gh is not logged in`이고 로그인은 `Refused`이며, `gh_retry` 동안은 gh를 다시
    /// 띄우지 않아요. gh가 로그인을 판단한 것과 다른 토큰을 주면 그 판단을 잊어요. gh에 묻는 것은 한 번에 하나예요.
    fn lease(&self) -> Result<Lease, SourceError> {
        let (generation, (mode, token)) = {
            let memory = lock(&self.memory);
            (memory.generation, self.cred())
        };
        match (mode, token) {
            (Mode::Token, Some(token)) => Ok(Lease {
                token,
                generation,
                from_gh: false,
            }),
            (Mode::Gh, _) => {
                let cached = |memory: &Memory| -> Option<Result<Lease, SourceError>> {
                    if memory.generation != generation {
                        return Some(Err(SourceError::service(Kind::Key, NOT_CONNECTED)));
                    }
                    if let Some(token) = &memory.gh_token {
                        return Some(Ok(Lease {
                            token: token.clone(),
                            generation,
                            from_gh: true,
                        }));
                    }
                    if memory.gh_quiet_until.is_some_and(|t| (self.clock)() < t) {
                        return Some(Err(SourceError::service(Kind::Key, NOT_LOGGED_IN)));
                    }
                    None
                };
                // 잠금은 cached 안에서만 쥐어요: 다시 하기(retry_after_switch)가 memory를 다시 잠가요
                let first = cached(&lock(&self.memory));
                if let Some(got) = first {
                    return got.or_else(|e| self.retry_after_switch(e));
                }
                // 한 번에 하나만 gh에 물어요. 기다린 뒤에는 앞 스레드가 받은 것을 써요
                let one = lock(&self.gh_lease);
                let second = cached(&lock(&self.memory));
                if let Some(got) = second {
                    drop(one); // 다시 하기 전에 놓아요(새 연결의 lease가 gh_lease를 잡아요)
                    return got.or_else(|e| self.retry_after_switch(e));
                }
                // gh는 memory 잠금 밖에서 물어요(키체인을 읽느라 느릴 수 있어요)
                let got = self.gh_token.token();
                let mut memory = lock(&self.memory);
                let same = memory.generation == generation;
                match got {
                    Ok(token) => {
                        if same {
                            memory.gh_token = Some(token.clone());
                            memory.gh_quiet_until = None;
                            if memory.judged.as_deref() != Some(token.as_str()) {
                                memory.login = None;
                                memory.judged = None;
                            }
                        }
                        Ok(Lease {
                            token,
                            generation,
                            from_gh: true,
                        })
                    }
                    Err(_) => {
                        if same {
                            memory.login = Some(LoginState::Refused);
                            memory.judged = None;
                            memory.gh_quiet_until = Some((self.clock)() + self.timeouts.gh_retry);
                        }
                        Err(SourceError::service(Kind::Key, NOT_LOGGED_IN))
                    }
                }
            }
            _ => Err(SourceError::service(Kind::Key, NOT_CONNECTED)),
        }
    }

    /// 묻는 사이에 연결이 바뀌었으면(세대가 다름) 새 연결로 다시 해요. 아니면 그 오류 그대로예요.
    fn retry_after_switch(&self, e: SourceError) -> Result<Lease, SourceError> {
        match &e {
            SourceError::Service { message, .. } if message == NOT_CONNECTED => self.lease(),
            _ => Err(e),
        }
    }

    /// gh에서 빌린 토큰이 거절됐어요(401, bad credentials의 403): 잊어서 다음 호출이 gh에 다시 물어요.
    fn refused(&self, lease: &Lease) {
        if !lease.from_gh {
            return;
        }
        let mut memory = lock(&self.memory);
        if memory.generation == lease.generation
            && memory.gh_token.as_deref() == Some(lease.token.as_str())
        {
            memory.gh_token = None;
        }
    }

    /// 답이 토큰을 거절했는지 보고, 그렇다면 gh 토큰을 잊어요.
    fn judge(&self, lease: &Lease, status: u16, body: &[u8]) {
        if status == 401 || (status == 403 && says_bad_credentials(body)) {
            self.refused(lease);
        }
    }

    /// 요청 하나예요. 상태가 무엇이든 `Answer`이고, 연결 실패·시간 초과는 network예요.
    fn get(
        &self,
        lease: &Lease,
        path: &str,
        headers: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<Answer, SourceError> {
        let answer = self.http.get(
            &lease.token,
            Target::Path(path),
            Accept::Json,
            headers,
            timeout,
        )?;
        self.judge(lease, answer.status, &answer.body);
        Ok(answer)
    }

    /// `GET user`로 본 로그인이에요(스펙 6-2). `Known`·`Refused`는 세대마다 기억하고 `Unknown`은 기억하지 않아요.
    /// 연결이 없으면 묻지 않고 `Unknown`이에요.
    pub fn login_state(&self) -> LoginState {
        let mode = self.mode();
        if mode == Mode::None {
            return LoginState::Unknown;
        }
        // gh 모드는 먼저 토큰을 빌려요: 대개 기억한 것이고, 잊었으면(401 뒤, gh 실패 60 s 뒤) gh에 다시 물어 다른
        // 토큰이면 앞 판단을 잊어요. 실패하면 lease가 Refused를 기억했어요
        let early = if mode == Mode::Gh {
            match self.lease() {
                Ok(lease) => Some(lease),
                Err(_) => {
                    return if self.connected() {
                        LoginState::Refused
                    } else {
                        LoginState::Unknown
                    };
                }
            }
        } else {
            None
        };
        let known = lock(&self.memory).login.clone();
        if let Some(state) = known {
            return state;
        }
        let lease = match early.map_or_else(|| self.lease(), Ok) {
            Ok(lease) => lease,
            // none으로 막 바뀌었어요
            Err(_) => {
                return if self.connected() {
                    LoginState::Refused
                } else {
                    LoginState::Unknown
                };
            }
        };
        let Ok(answer) = self.get(&lease, "user", &[], self.timeouts.login) else {
            return LoginState::Unknown;
        };
        let body: Option<Value> = serde_json::from_slice(&answer.body).ok();
        let message = body
            .as_ref()
            .and_then(|b| b.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let state = match answer.status {
            200 => match body
                .as_ref()
                .and_then(|b| b.get("login"))
                .and_then(Value::as_str)
            {
                Some(name) if !name.is_empty() => LoginState::Known(name.to_string()),
                _ => return LoginState::Unknown,
            },
            401 => LoginState::Refused,
            403 if message.contains("bad credentials") => LoginState::Refused,
            _ => return LoginState::Unknown,
        };
        let mut memory = lock(&self.memory);
        if memory.generation == lease.generation {
            memory.login = Some(state.clone());
            memory.judged = Some(lease.token.clone());
        }
        state
    }

    /// 로그인한 GitHub 계정이에요(`login_state`가 `Known`일 때만).
    pub fn login(&self) -> Option<String> {
        self.login_state().name()
    }

    /// GET의 JSON 답이에요. 2xx가 아니거나, 연결이 끊기거나, 답이 JSON이 아니면(잘림) 2×(n+1) s 쉬고
    /// `tries`번까지 물어요. 마지막 실패는 스펙 4-4의 종류예요. 토큰을 얻지 못하면(연결 없음, gh 로그아웃) 다시 묻지
    /// 않아요.
    pub fn json(&self, path: &str, tries: u32) -> Result<Value, SourceError> {
        self.retried(tries, |lease| {
            checked(&self.get(lease, path, &[], self.timeouts.request)?)
        })
    }

    /// `f`를 `tries`번까지 해요: 실패하면 2×(n+1) s 쉬고 다시예요. 토큰은 해마다 새로 얻어요(401 뒤 gh가 다시
    /// 줄 수 있어요). 토큰을 얻지 못하면 바로 그 오류예요.
    fn retried<T>(
        &self,
        tries: u32,
        f: impl Fn(&Lease) -> Result<T, SourceError>,
    ) -> Result<T, SourceError> {
        let tries = tries.max(1);
        let mut attempt = 0;
        loop {
            let lease = self.lease()?;
            let failed = match f(&lease) {
                Ok(v) => return Ok(v),
                Err(e) => e,
            };
            if attempt + 1 >= tries {
                return Err(failed);
            }
            (self.pause)(Duration::from_secs(2 * u64::from(attempt + 1)));
            attempt += 1;
        }
    }

    /// GET 한 번의 (HTTP 상태, 소문자 이름의 머리, JSON 몸통 또는 None)이에요. 상태가 무엇이든
    /// 한 번만 물어요. 연결 실패·시간 초과(`timeout`, 없으면 요청 하나의 시간)만 오류(network)예요. 몸통은 찍지 않아요.
    pub fn status(
        &self,
        path: &str,
        headers: &[(&str, &str)],
        timeout: Option<Duration>,
    ) -> Result<Status, SourceError> {
        let lease = self.lease()?;
        let answer = self.get(
            &lease,
            path,
            headers,
            timeout.unwrap_or(self.timeouts.request),
        )?;
        let body = if answer.body.iter().all(u8::is_ascii_whitespace) {
            None
        } else {
            serde_json::from_slice(&answer.body).ok()
        };
        Ok((answer.status, answer.headers, body))
    }

    /// GET 한 번의 날 바이트예요(blob, `Accept::Raw`). 2xx가 아니면 `kind_of`의 오류이고, 한 번만 물어요.
    pub fn bytes(
        &self,
        path: &str,
        accept: Accept,
        timeout: Duration,
    ) -> Result<Vec<u8>, SourceError> {
        let lease = self.lease()?;
        let answer = self
            .http
            .get(&lease.token, Target::Path(path), accept, &[], timeout)?;
        self.judge(&lease, answer.status, &answer.body);
        if (200..300).contains(&answer.status) {
            return Ok(answer.body);
        }
        // 오류 몸통만 JSON으로 읽어요(파일 바이트는 JSON으로 읽지 않아요)
        let body: Option<Value> = serde_json::from_slice(&answer.body).ok();
        let (kind, message) = kind_of(answer.status, &answer.headers, body.as_ref()).unwrap_or((
            Kind::Down,
            format!("GitHub answered HTTP {}", answer.status),
        ));
        Err(SourceError::Service { kind, message })
    }

    /// GET 한 번의 몸통을 흘려 읽는 것이에요(tarball). 2xx가 아니면 몸통(앞 64 KiB)을 읽어 `kind_of`의 오류예요.
    /// 한 번만 물어요. 읽다 난 오류의 글은 부르는 쪽이 고정 글로 바꿔요(`http::Http::open`).
    pub fn stream(
        &self,
        path: &str,
        accept: Accept,
        timeout: Duration,
    ) -> Result<Box<dyn std::io::Read + Send>, SourceError> {
        use std::io::Read as _;
        let lease = self.lease()?;
        let opened = self.http.open(&lease.token, path, accept, timeout)?;
        if (200..300).contains(&opened.status) {
            return Ok(opened.body);
        }
        let mut body = Vec::new();
        // 오류 몸통은 작아요. 읽지 못해도 상태로 가려요
        let _ = opened.body.take(64 * 1024).read_to_end(&mut body);
        self.judge(&lease, opened.status, &body);
        let parsed: Option<Value> = serde_json::from_slice(&body).ok();
        let (kind, message) = kind_of(opened.status, &opened.headers, parsed.as_ref()).unwrap_or((
            Kind::Down,
            format!("GitHub answered HTTP {}", opened.status),
        ));
        Err(SourceError::Service { kind, message })
    }

    /// `GET path` 한 번의 (성공, 몸통의 `message`)예요. 그 글은 승인·SSO 가르기에만 안에서 써요.
    ///
    /// `none` 모드에서는 key 오류를 던지지 않고 `(false, "GitHub is not connected")`를 돌려줘요. 그래서
    /// 연결 확인으로 쓰면 안 돼요(연결은 `connected()`로 봐요).
    pub fn probe(&self, path: &str) -> (bool, String) {
        let lease = match self.lease() {
            Ok(lease) => lease,
            Err(e) => return (false, e.detail()),
        };
        match self.get(&lease, path, &[], self.timeouts.probe) {
            Ok(answer) => {
                let message = serde_json::from_slice::<Value>(&answer.body)
                    .ok()
                    .and_then(|b| b.get("message")?.as_str().map(str::to_string))
                    .unwrap_or_default();
                ((200..300).contains(&answer.status), message)
            }
            Err(_) => (false, String::new()),
        }
    }

    /// 목록 전부(`Link`를 따라간 모든 쪽)를 `tries`번까지 물어요(`json`과 같은 다시 묻기).
    fn list(
        &self,
        path: &str,
        extra: &[(&str, &str)],
        tries: u32,
    ) -> Result<Vec<Value>, SourceError> {
        // 쪽마다 `get`처럼 401과 bad credentials의 403을 거절로 봐요
        self.retried(tries, |lease| {
            self.http.pages_seen(
                &lease.token,
                path,
                extra,
                self.timeouts.request,
                &|status, body| self.judge(lease, status, body),
            )
        })
    }

    /// 목록의 모든 항목이에요. 한 번만 물어요(여기서는 빈 목록도 진짜 답이에요). `path`에 `per_page`는 넣지 않아요.
    pub fn pages(&self, path: &str) -> Result<Vec<Value>, SourceError> {
        self.list(path, &[], 1)
    }

    /// 목록의 모든 쪽이에요. 실패는 `json`처럼 3번까지 묻고, 빈 답은 1+n s 쉬고 `tries`번까지 다시
    /// 물어요: 같은 요청에 목록이 비어 오는 때가 있었고(2026-10-23), 잃은 목록은 저장한 것을 지울 수 있어요. `since`
    /// 목록은 한 번만 물어요(새것 없음이 보통의 답이에요).
    pub fn rest(
        &self,
        path: &str,
        since: Option<&str>,
        tries: u32,
    ) -> Result<Vec<Value>, SourceError> {
        let mut tries = tries.max(1);
        let mut extra = Vec::new();
        if let Some(since) = since.filter(|s| !s.is_empty()) {
            extra.push(("since", since));
            tries = 1;
        }
        let mut attempt = 0;
        loop {
            let items = self.list(path, &extra, 3)?;
            if !items.is_empty() || attempt + 1 >= tries {
                return Ok(items);
            }
            (self.pause)(Duration::from_secs(1 + u64::from(attempt)));
            attempt += 1;
        }
    }

    /// GraphQL 질의 하나의 `repository` 객체예요. `POST graphql`, 본문은 `{"query", "variables":
    /// {"owner", "name"}}`예요. 200이고 `data`가 있으면 부분 오류(별명 하나가 NOT_FOUND)여도 그대로 써요(실패한 별명은
    /// null). 아니면 2×(n+1) s 쉬고 `GRAPHQL_TRIES`번까지 묻고, 마지막 실패는 스펙 4-4의 종류(200인데 `data`가
    /// 없으면 down)예요.
    pub fn graphql(
        &self,
        query: &str,
        owner: &str,
        name: &str,
    ) -> Result<Map<String, Value>, SourceError> {
        let body = json!({"query": query, "variables": {"owner": owner, "name": name}});
        self.retried(GRAPHQL_TRIES, |lease| {
            let answer =
                self.http
                    .post_json(&lease.token, "graphql", &body, self.timeouts.request)?;
            self.judge(lease, answer.status, &answer.body);
            let got: Option<Value> = serde_json::from_slice(&answer.body).ok();
            if answer.status != 200 {
                let (kind, message) = kind_of(answer.status, &answer.headers, got.as_ref())
                    .unwrap_or((
                        Kind::Down,
                        format!("GitHub answered HTTP {}", answer.status),
                    ));
                return Err(SourceError::Service { kind, message });
            }
            let Some(Value::Object(mut data)) = got.and_then(|v| v.get("data").cloned()) else {
                return Err(SourceError::Json(
                    "GitHub's GraphQL answer has no data".to_string(),
                ));
            };
            // `repository`가 없거나 거짓 값이면 빈 객체, 객체가 아닌 참 값은 오류예요
            match data.shift_remove("repository") {
                Some(Value::Object(repo)) => Ok(repo),
                Some(v) if py::truthy(&v) => Err(SourceError::Json(
                    "GitHub's GraphQL repository is not an object".to_string(),
                )),
                _ => Ok(Map::new()),
            }
        })
    }

    /// 사용자의 저장소(최근에 푸시한 것부터)와, 들어가려면 승인이나 SSO 로그인이 필요한 조직이에요.
    pub fn repos(&self) -> Result<Value, SourceError> {
        let mut items = self.pages(REPOS)?;
        // 늦은 것부터예요. 안정 정렬이라 시각이 같으면 받은 순서예요
        items.sort_by(|a, b| pushed_at(b).cmp(pushed_at(a)));
        let mut out = Vec::with_capacity(items.len());
        for r in &items {
            let Some(full_name) = r.get("full_name") else {
                return Err(SourceError::Json(
                    "a repository without full_name".to_string(),
                ));
            };
            out.push(json!({
                "full_name": full_name,
                "private": r.get("private").is_some_and(py::truthy),
                "default_branch": r.get("default_branch").cloned().unwrap_or(Value::Null),
                "pushed_at": r.get("pushed_at").cloned().unwrap_or(Value::Null),
            }));
        }
        let orgs: Vec<String> = self
            .pages("user/orgs")?
            .iter()
            .filter_map(|o| o.get("login")?.as_str())
            .filter(|o| ORG_LOGIN.is_match(o))
            .map(str::to_string)
            .collect();
        let (mut approval, mut sso) = (Vec::new(), Vec::new());
        for (login, (ok, err)) in orgs.iter().zip(self.probe_orgs(&orgs)) {
            if ok {
                continue;
            }
            if err.contains("OAuth App access restrictions") {
                approval.push(login.clone());
            } else if err.contains("SAML enforcement") {
                sso.push(login.clone());
            }
        }
        Ok(json!({"repos": out, "needs_approval": approval, "needs_sso": sso}))
    }

    /// 조직마다 비공개 저장소 하나를 물어요. 4개씩 돌고 결과는 조직 순서예요. 실패한 확인은 목록을 실패시키지 않아요.
    fn probe_orgs(&self, orgs: &[String]) -> Vec<(bool, String)> {
        let results: Vec<Mutex<(bool, String)>> = orgs
            .iter()
            .map(|_| Mutex::new((false, String::new())))
            .collect();
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..PROBE_WORKERS.min(orgs.len()) {
                s.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(org) = orgs.get(i) else { break };
                        let got = self.probe(&format!("orgs/{org}/repos?type=private&per_page=1"));
                        *lock(&results[i]) = got;
                    }
                });
            }
        });
        results
            .into_iter()
            .map(|m| m.into_inner().unwrap_or_else(PoisonError::into_inner))
            .collect()
    }

    /// 저장소의 브랜치예요. 기본 브랜치가 맨 앞이고 나머지는 GitHub 순서예요.
    pub fn branches(&self, repo: &str) -> Result<Value, SourceError> {
        let default = self
            .json(&format!("repos/{repo}"), 1)?
            .get("default_branch")
            .cloned()
            .unwrap_or(Value::Null);
        let mut names = Vec::new();
        for b in self.pages(&format!("repos/{repo}/branches"))? {
            match b.get("name") {
                Some(name) => names.push(name.clone()),
                None => return Err(SourceError::Json("a branch without name".to_string())),
            }
        }
        names.sort_by_key(|n| *n != default); // 안정 정렬: 기본이 앞, 나머지는 받은 순서
        Ok(json!({"branches": names, "default": default}))
    }
}

/// 몸통의 `message`에 `bad credentials`가 있어요(403으로 오는 토큰 거절).
fn says_bad_credentials(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|b| b.get("message")?.as_str().map(str::to_lowercase))
        .is_some_and(|m| m.contains("bad credentials"))
}

/// `pushed_at`이에요(없거나 글이 아니면 빈 글이에요).
fn pushed_at(r: &Value) -> &str {
    r.get("pushed_at").and_then(Value::as_str).unwrap_or("")
}

/// `owner/name`이에요: 이름은 `.`이나 `..`가 아니에요(`...`는 돼요).
pub fn valid_repo_name(name: &str) -> bool {
    REPO_NAME.is_match(name)
        && name
            .split_once('/')
            .is_some_and(|(_, n)| n != "." && n != "..")
}

/// GitHub의 호출 제한이에요: 429, 또는 남은 요청이 없거나(x-ratelimit-remaining: 0) 메시지가 그것을 말하는 403이에요
/// (2차 제한은 요청이 남아 있어도 403을 보내요).
pub fn rate_limited(status: u16, headers: &HashMap<String, String>, body: Option<&Value>) -> bool {
    let message = body.and_then(|b| b.get("message")).and_then(Value::as_str);
    status == 429
        || (status == 403
            && (headers.get("x-ratelimit-remaining").map(String::as_str) == Some("0")
                || message.is_some_and(|m| m.to_lowercase().contains("rate limit"))))
}
