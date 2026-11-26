//! A fake GitHub REST server (spec 7-1). A small HTTP/1.1 server on `std::net::TcpListener` (127.0.0.1:0), with one
//! thread per connection, one request each, and `Connection: close`. The engine's real client
//! (`github::http::Http`) asks it over loopback.
//!
//! # Setting replies
//!
//! | What you set | Requests it answers |
//! |---|---|
//! | `route(path, reply)`, `route_seq(path, replies)` | GET `path` ("Route matching" below) |
//! | `status(path, code, headers, body)`, `status_seq` | The same table (a `route` set with `Reply::Http`) |
//! | `route_with(path, f)` | GET `path` (route matching as in `route`): `f("path?query", If-None-Match)` |
//! | `status_with(f)` | Every GET not in the table: `f("path?query", If-None-Match)` |
//! | `graphql(f)` | `POST graphql`: `f(query, variables)` → (status, JSON) |
//! | `login(reply)` | `GET user` |
//!
//! - **Route matching**: a route is a path without the leading `/` (as sent, percent-encoding included) and may carry
//!   a query. It matches when the request path is the same and every query value of the route is in the request. If
//!   several match, the one with more query values wins, then the one added later.
//!   `per_page`, `page` and `after` are ignored for matching. Adding the same route again replaces it. Values are not
//!   decoded and are compared as the text sent: the client sends `since=2026-10-26T01:02:03Z` as
//!   `since=2026-10-26T01%3A02%3A03Z`, so to match on that value write the route encoded too, like
//!   `route("repos/o/r/issues?since=2026-10-26T01%3A02%3A03Z", …)`.
//!   `route("repos/o/r/issues?state=all", …)` also matches `…/issues?state=all&per_page=100&since=…`.
//! - **Paging**: if a `route` reply is `Reply::Json(array)` and the request has `per_page`, the server splits it. When
//!   there is a next page it gives `Link: <{url}repositories/{id}/{끝}?{쿼리}&after=c{n}&page={n}>; rel="next"`
//!   (the form of appendix A-6). `{id}` is numbered from 1 per `repos/{owner}/{name}`, and `{끝}` is the path after it
//!   (the whole route for routes not under `repos/…`). `{쿼리}` is the request's query without `page`/`after`.
//!   Requests to `repositories/{id}/…` are mapped back to the original route and answered.
//! - `route`, `status` and `login` repeat one reply forever; `*_seq` answers each first-page request in turn and then
//!   repeats the last one (next-page requests keep splitting the same reply).
//! - Requests with no reply set get 500 and are recorded in `unanswered()` as `"METHOD path?query"`. Reading
//!   `unanswered()` clears it. If it is not empty when `FakeGitHub` is dropped (and the thread is not already
//!   panicking), it panics.
//!
//! `Reply`: `Json(v)` (200, JSON), `Http { code, headers, body }` (as is), `Slow(d, r)` (wait `d`, then `r`), `Close`
//! (drop the connection without replying).
//!
//! # Records
//!
//! `calls()` is one `Call` per request. The token value is not recorded; only whether it was
//! `Authorization: Bearer {FakeGitHub::TOKEN}` (`has_token`). JSON bodies (GraphQL queries and variables) are kept
//! in `body`.
//!
//! # Repo content (`repo(name, id)` → `FakeRepo`)
//!
//! Table routes come first (so a test can swap out just one route); anything not in the table is answered by the repo
//! content:
//!
//! | Request | Reply |
//! |---|---|
//! | `repos/{name}` | `{"id", "name", "full_name", "default_branch", "private"}` |
//! | `repos/{name}/git/ref/heads/{branch}` | `{"ref", "object": {"sha", "type"}}` of the branch with that name (path decoded for comparison), else 404 |
//! | `repos/{name}/git/trees/{sha}[?recursive=1]` | Entries of the commit or tree (deliberately in reverse path order); unknown SHA is 422 |
//! | `repos/{name}/git/blobs/{sha}` | Raw bytes for a raw `Accept`, otherwise base64 JSON; unknown SHA is 404 |
//! | `repos/{name}/tarball/{sha}` | 303 to the second listener (codeload) `…/legacy.tar.gz/{sha}?token=x`; unknown commit is 404 |
//!
//! - `push(branch, files)` makes a new commit from the full file set (path, mode, bytes). Old commits stay. Blob SHAs
//!   are real git values; tree and commit SHAs are made with sha1 (not real git objects). Mode 120000 is a link (the
//!   bytes are the target), 160000 is a submodule (`type: commit` in the tree, an empty folder in the tarball).
//! - With `truncate_trees(true)`, `recursive=1` replies are `truncated: true` and give only one top-level entry.
//! - tarball: top folder `{owner}-{repo}-{sha}/`, a pax global header, folder entries, regular files and links.
//!   `tarball_differs(path, bytes)` changes that file's bytes (mimicking export-subst), `tarball_link(path, target)`
//!   turns that file into a link, and `tarball_extra(raw, bytes)` appends a regular file with the name as given (like
//!   `../x`). With `codeload_cut(true)` codeload sends only half the body and drops the connection.
//! - With `offline(true)`, content requests (`git/…`, `tarball/…`) are dropped without a reply. Repo names are
//!   case-insensitive.
//! - codeload remembers if `Authorization` arrives (`codeload_saw_authorization`). The number of requests it received
//!   is `codeload_served`. codeload requests are not put in `calls()`.
//!
//! # gh token fakes
//!
//! `FakeGhToken` (answers in turn and counts calls) and `PanicGhToken` (panics when called: guards that token and
//! none modes never ask gh). Plug them in like `GitHub::new(mode, &fake.url(), Arc::new(PanicGhToken))`.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use flate2::Compression;
use flate2::write::GzEncoder;
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::error::SourceError;
use crate::github::content::blob_sha;
use crate::github::gh_token::{GhToken, not_logged_in};

/// 가짜 서버의 답 하나예요.
#[derive(Clone, Debug)]
pub enum Reply {
    /// 200, `application/json`이에요. `route`의 배열은 `per_page`대로 나눠요.
    Json(Value),
    /// 이 상태, 머리, 몸통 그대로예요.
    Http {
        code: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// 이만큼 쉬고 답해요(시간 초과 시험).
    Slow(Duration, Box<Reply>),
    /// 답하지 않고 연결을 끊어요.
    Close,
}

/// 받은 요청 하나예요. 토큰 값은 없어요.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub method: String,
    /// 앞 `/` 없는 경로(보낸 그대로)예요.
    pub path: String,
    /// `?` 뒤(없으면 빈 글)예요.
    pub query: String,
    pub has_token: bool,
    pub if_none_match: Option<String>,
    pub accept: Option<String>,
    pub user_agent: Option<String>,
    pub api_version: Option<String>,
    /// JSON 몸통이에요(GraphQL의 `{"query", "variables"}`). 몸통이 없거나 JSON이 아니면 None이에요.
    pub body: Option<Value>,
}

type StatusFn = dyn Fn(&str, Option<&str>) -> Reply + Send + Sync;
type GraphqlFn = dyn Fn(&str, &Value) -> (u16, Value) + Send + Sync;

/// 길 하나의 답이에요: 차례로 쓰고 마지막을 되풀이해요. `served`는 지금 나누고 있는 답이에요.
struct Script {
    replies: Vec<Reply>,
    next: usize,
    served: Option<Reply>,
}

struct Route {
    path: String,
    query: Vec<(String, String)>,
    order: usize,
    script: Script,
    /// `route_with`의 함수예요. 있으면 `script` 대신 이것이 답해요.
    dynamic: Option<Arc<StatusFn>>,
}

#[derive(Default)]
struct State {
    routes: Vec<Route>,
    added: usize,
    status_with: Option<Arc<StatusFn>>,
    graphql: Option<Arc<GraphqlFn>>,
    login: Option<Reply>,
    calls: Vec<Call>,
    unanswered: Vec<String>,
    /// `repos/{owner}/{name}` → 다음 쪽 주소의 id예요.
    repo_ids: Vec<String>,
    /// 저장소 내용이에요(`FakeGitHub::repo`).
    repos: BTreeMap<String, RepoState>,
    codeload_auth: bool,
    codeload_served: usize,
}

/// 트리 항목 하나예요(바로 아래만).
#[derive(Clone)]
struct Item {
    name: String,
    mode: String,
    kind: &'static str,
    sha: String,
}

/// 가짜 저장소 하나의 내용이에요.
struct RepoState {
    id: u64,
    default_branch: String,
    branches: HashMap<String, String>,
    /// 커밋 SHA → 맨 위 트리 SHA
    commits: HashMap<String, String>,
    /// 커밋 SHA → 그 커밋의 파일(경로, 모드, 바이트), tarball용
    files: HashMap<String, Vec<(String, String, Vec<u8>)>>,
    trees: HashMap<String, Vec<Item>>,
    blobs: HashMap<String, Vec<u8>>,
    pushes: u64,
    truncate: bool,
    differs: HashMap<String, Vec<u8>>,
    links: HashMap<String, String>,
    extra: Vec<(String, Vec<u8>)>,
    cut: bool,
    offline: bool,
}

struct Shared {
    state: Mutex<State>,
    url: String,
    /// 두 번째 리스너(codeload)의 주소예요.
    codeload: String,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// 가짜 GitHub 서버예요(`FakeGitHub::start()`는 `Arc<FakeGitHub>`).
pub struct FakeGitHub {
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    port: u16,
    codeload_port: u16,
}

impl FakeGitHub {
    /// 가짜가 맞다고 보는 토큰이에요.
    // not shaped like a real token: GitHub's push protection on the public repository would stop a `ghp_` one
    pub const TOKEN: &'static str = "fake-github-token-for-tests";

    /// 127.0.0.1의 빈 port에서 듣기 시작해요.
    pub fn start() -> Arc<FakeGitHub> {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake GitHub");
        let port = listener.local_addr().expect("fake GitHub address").port();
        let codeload = TcpListener::bind("127.0.0.1:0").expect("bind the fake codeload");
        let codeload_port = codeload.local_addr().expect("fake codeload address").port();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            url: format!("http://127.0.0.1:{port}/"),
            codeload: format!("http://127.0.0.1:{codeload_port}/"),
        });
        let stop = Arc::new(AtomicBool::new(false));
        listen(listener, shared.clone(), stop.clone(), serve);
        listen(codeload, shared.clone(), stop.clone(), serve_codeload);
        Arc::new(FakeGitHub {
            shared,
            stop,
            port,
            codeload_port,
        })
    }

    /// `http://127.0.0.1:<port>/`예요.
    pub fn url(&self) -> String {
        self.shared.url.clone()
    }

    pub fn route(&self, path: &str, reply: Reply) {
        self.route_seq(path, vec![reply]);
    }

    pub fn route_seq(&self, path: &str, replies: Vec<Reply>) {
        assert!(!replies.is_empty(), "route_seq needs at least one reply");
        self.add_route(path, replies, None);
    }

    /// 이 길(맞추기는 `route`와 같아요)에 `f("path?query", If-None-Match)`로 답해요. 요청마다 답이 바뀌어야 하는 길
    /// (ETag 확인)을 다른 길보다 앞세울 때 써요. 배열 답도 나누지 않아요.
    pub fn route_with(
        &self,
        path: &str,
        f: impl Fn(&str, Option<&str>) -> Reply + Send + Sync + 'static,
    ) {
        self.add_route(path, vec![Reply::Close], Some(Arc::new(f)));
    }

    fn add_route(&self, path: &str, replies: Vec<Reply>, dynamic: Option<Arc<StatusFn>>) {
        let (p, q) = split(path.trim_start_matches('/'));
        let query = without_paging(&pairs(q));
        let mut st = self.shared.lock();
        st.added += 1;
        let order = st.added;
        st.routes.retain(|r| !(r.path == p && r.query == query));
        st.routes.push(Route {
            path: p.to_string(),
            query,
            order,
            script: Script {
                replies,
                next: 0,
                served: None,
            },
            dynamic,
        });
    }

    pub fn status(&self, path: &str, code: u16, headers: &[(&str, &str)], body: Option<Value>) {
        self.route(path, http_reply(code, headers, body));
    }

    pub fn status_seq(&self, path: &str, replies: Vec<Reply>) {
        self.route_seq(path, replies);
    }

    pub fn status_with(&self, f: impl Fn(&str, Option<&str>) -> Reply + Send + Sync + 'static) {
        self.shared.lock().status_with = Some(Arc::new(f));
    }

    pub fn graphql(&self, f: impl Fn(&str, &Value) -> (u16, Value) + Send + Sync + 'static) {
        self.shared.lock().graphql = Some(Arc::new(f));
    }

    pub fn login(&self, reply: Reply) {
        self.shared.lock().login = Some(reply);
    }

    pub fn calls(&self) -> Vec<Call> {
        self.shared.lock().calls.clone()
    }

    pub fn clear_calls(&self) {
        self.shared.lock().calls.clear();
    }

    /// 답을 정하지 않은 요청이에요(`"METHOD path?query"`). 읽으면 비워요.
    pub fn unanswered(&self) -> Vec<String> {
        std::mem::take(&mut self.shared.lock().unanswered)
    }

    /// 저장소 `name`(`owner/repo`)의 내용이에요. 처음이면 빈 저장소(기본 브랜치 `main`)를 만들고, 있으면 id만 바꿔요.
    pub fn repo(&self, name: &str, id: u64) -> FakeRepo {
        let mut st = self.shared.lock();
        st.repos
            .entry(name.to_string())
            .and_modify(|r| r.id = id)
            .or_insert_with(|| RepoState {
                id,
                default_branch: "main".to_string(),
                branches: HashMap::new(),
                commits: HashMap::new(),
                files: HashMap::new(),
                trees: HashMap::new(),
                blobs: HashMap::new(),
                pushes: 0,
                truncate: false,
                differs: HashMap::new(),
                links: HashMap::new(),
                extra: Vec::new(),
                cut: false,
                offline: false,
            });
        FakeRepo {
            shared: self.shared.clone(),
            name: name.to_string(),
        }
    }

    /// codeload가 `Authorization` 머리를 받은 적이 있는지예요.
    pub fn codeload_saw_authorization(&self) -> bool {
        self.shared.lock().codeload_auth
    }

    /// codeload가 받은 요청 수예요.
    pub fn codeload_served(&self) -> usize {
        self.shared.lock().codeload_served
    }
}

/// 가짜 저장소 하나예요(`FakeGitHub::repo`).
pub struct FakeRepo {
    shared: Arc<Shared>,
    name: String,
}

impl FakeRepo {
    fn with<T>(&self, f: impl FnOnce(&mut RepoState) -> T) -> T {
        let mut st = self.shared.lock();
        f(st.repos.get_mut(&self.name).expect("a fake repo"))
    }

    /// 이 파일 전체(경로, 모드, 바이트)로 `branch`에 새 커밋을 만들어요. 커밋 SHA예요.
    pub fn push(&self, branch: &str, files: &[(&str, &str, &[u8])]) -> String {
        self.with(|r| {
            r.pushes += 1;
            let mut list: Vec<(String, String, Vec<u8>)> = files
                .iter()
                .map(|(p, m, b)| (p.to_string(), m.to_string(), b.to_vec()))
                .collect();
            list.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            let root = build_tree(r, "", &list);
            let commit = sha1_hex(&format!("commit {root} {} {branch}", r.pushes));
            r.commits.insert(commit.clone(), root);
            r.files.insert(commit.clone(), list);
            r.branches.insert(branch.to_string(), commit.clone());
            commit
        })
    }

    /// `repos/{name}`의 `default_branch`예요.
    pub fn default_branch(&self, b: &str) {
        self.with(|r| r.default_branch = b.to_string());
    }

    /// 켜면 `recursive=1` 트리 답이 잘려요(`truncated: true`).
    pub fn truncate_trees(&self, on: bool) {
        self.with(|r| r.truncate = on);
    }

    /// tarball 안의 이 파일 바이트를 바꿔요(트리와 어긋나요).
    pub fn tarball_differs(&self, path: &str, bytes: &[u8]) {
        self.with(|r| r.differs.insert(path.to_string(), bytes.to_vec()));
    }

    /// tarball 안의 이 파일을 `target`을 가리키는 링크로 바꿔요.
    pub fn tarball_link(&self, path: &str, target: &str) {
        self.with(|r| r.links.insert(path.to_string(), target.to_string()));
    }

    /// tarball 끝에 이 이름 그대로의 보통 파일을 더해요(`../x`, `/x`, 맨 위 폴더 밖).
    pub fn tarball_extra(&self, raw_path: &str, bytes: &[u8]) {
        self.with(|r| r.extra.push((raw_path.to_string(), bytes.to_vec())));
    }

    /// 켜면 codeload가 몸통의 절반만 보내고 연결을 끊어요.
    pub fn codeload_cut(&self, on: bool) {
        self.with(|r| r.cut = on);
    }

    /// 켜면 내용 요청(`git/ref`, `git/trees`, `git/blobs`, `tarball`)이 답 없이 끊겨요: 저장소 자체(`repos/{name}`)와
    /// 목록은 그대로 답해요(코드만 받지 못하는 오프라인).
    pub fn offline(&self, on: bool) {
        self.with(|r| r.offline = on);
    }

    /// codeload가 `Authorization`을 받은 적이 있는지예요(`FakeGitHub::codeload_saw_authorization`과 같아요).
    pub fn codeload_saw_authorization(&self) -> bool {
        self.shared.lock().codeload_auth
    }
}

fn sha1_hex(s: &str) -> String {
    hex::encode(Sha1::digest(s.as_bytes()))
}

/// `prefix` 아래 파일들(정렬된 전체 목록)로 트리를 만들고 그 SHA예요. 하위 트리와 blob을 저장소에 넣어요.
fn build_tree(r: &mut RepoState, prefix: &str, files: &[(String, String, Vec<u8>)]) -> String {
    let mut items: Vec<Item> = Vec::new();
    let mut folders: Vec<String> = Vec::new();
    for (path, mode, bytes) in files {
        let Some(rest) = path.strip_prefix(prefix) else {
            continue;
        };
        match rest.split_once('/') {
            Some((dir, _)) => {
                if !folders.iter().any(|f| f == dir) {
                    folders.push(dir.to_string());
                }
            }
            None => {
                let sha = blob_sha(bytes);
                let kind = if mode == "160000" { "commit" } else { "blob" };
                if kind == "blob" {
                    r.blobs.insert(sha.clone(), bytes.clone());
                }
                items.push(Item {
                    name: rest.to_string(),
                    mode: mode.clone(),
                    kind,
                    sha,
                });
            }
        }
    }
    for dir in folders {
        let sha = build_tree(r, &format!("{prefix}{dir}/"), files);
        items.push(Item {
            name: dir,
            mode: "040000".to_string(),
            kind: "tree",
            sha,
        });
    }
    let listing: Vec<String> = items
        .iter()
        .map(|i| format!("{} {} {}", i.mode, i.name, i.sha))
        .collect();
    let sha = sha1_hex(&format!("tree {}", listing.join("\n")));
    r.trees.insert(sha.clone(), items);
    sha
}

impl Drop for FakeGitHub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // 듣는 스레드를 깨워 끝내요
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        let _ = TcpStream::connect(("127.0.0.1", self.codeload_port));
        let left = std::mem::take(&mut self.shared.lock().unanswered);
        if !left.is_empty() && !thread::panicking() {
            panic!("the fake GitHub got requests nobody answered: {left:?}");
        }
    }
}

fn http_reply(code: u16, headers: &[(&str, &str)], body: Option<Value>) -> Reply {
    let mut hs: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let body = match body {
        Some(b) => {
            hs.push(("Content-Type".into(), "application/json".into()));
            b.to_string().into_bytes()
        }
        None => Vec::new(),
    };
    Reply::Http {
        code,
        headers: hs,
        body,
    }
}

fn split(target: &str) -> (&str, &str) {
    target.split_once('?').unwrap_or((target, ""))
}

/// 쿼리를 (이름, 값)으로 나눠요. 값은 보낸 그대로(인코딩된 채)예요.
fn pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_string(), v.to_string())
        })
        .collect()
}

const PAGING: [&str; 3] = ["per_page", "page", "after"];

struct Request {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    let (path, query) = split(target.trim_start_matches('/'));
    Some(Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers,
        body,
    })
}

/// 연결마다 스레드 하나로 `handle`해요. `stop`이면 끝나요.
fn listen(
    listener: TcpListener,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    handle: fn(&Shared, TcpStream),
) {
    thread::spawn(move || {
        for stream in listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let s = shared.clone();
            thread::spawn(move || handle(&s, stream));
        }
    });
}

fn serve(shared: &Shared, mut stream: TcpStream) {
    let Some(req) = read_request(&stream) else {
        return;
    };
    let token = format!("Bearer {}", FakeGitHub::TOKEN);
    shared.lock().calls.push(Call {
        method: req.method.clone(),
        path: req.path.clone(),
        query: req.query.clone(),
        has_token: req.headers.get("authorization") == Some(&token),
        if_none_match: req.headers.get("if-none-match").cloned(),
        accept: req.headers.get("accept").cloned(),
        user_agent: req.headers.get("user-agent").cloned(),
        api_version: req.headers.get("x-github-api-version").cloned(),
        body: serde_json::from_slice(&req.body).ok(),
    });
    let reply = answer(shared, &req).unwrap_or_else(|| {
        let mut what = format!("{} {}", req.method, req.path);
        if !req.query.is_empty() {
            what.push('?');
            what.push_str(&req.query);
        }
        shared.lock().unanswered.push(what);
        http_reply(500, &[], None)
    });
    write_reply(&mut stream, reply);
}

fn write_reply(stream: &mut TcpStream, reply: Reply) {
    let (code, headers, body) = match reply {
        Reply::Close => return,
        Reply::Slow(d, r) => {
            thread::sleep(d);
            return write_reply(stream, *r);
        }
        Reply::Json(v) => (
            200,
            vec![("Content-Type".to_string(), "application/json".to_string())],
            v.to_string().into_bytes(),
        ),
        Reply::Http {
            code,
            headers,
            body,
        } => (code, headers, body),
    };
    let mut out = format!("HTTP/1.1 {code} {}\r\n", reason(code));
    for (k, v) in &headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(&body);
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        303 => "See Other",
        304 => "Not Modified",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        410 => "Gone",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

/// 요청의 답이에요. 답을 정하지 않았으면 None이에요.
fn answer(shared: &Shared, req: &Request) -> Option<Reply> {
    if req.method == "POST" && req.path == "graphql" {
        let f = shared.lock().graphql.clone()?;
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
        let query = body.get("query").and_then(Value::as_str).unwrap_or("");
        let vars = body.get("variables").cloned().unwrap_or(Value::Null);
        let (code, v) = f(query, &vars);
        return Some(http_reply(code, &[], Some(v)));
    }
    if req.method != "GET" {
        return None;
    }
    if req.path == "user"
        && let Some(r) = shared.lock().login.clone()
    {
        return Some(r);
    }
    let query = pairs(&req.query);
    let path = original_path(shared, &req.path)?;
    let page: usize = value(&query, "page")
        .and_then(|p| p.parse().ok())
        .unwrap_or(1);
    let routed = {
        let mut st = shared.lock();
        let found = st
            .routes
            .iter_mut()
            .filter(|r| r.path == path && r.query.iter().all(|kv| query.contains(kv)))
            .max_by_key(|r| (r.query.len(), r.order));
        found.map(|r| {
            if let Some(f) = &r.dynamic {
                return Err(f.clone());
            }
            let s = &mut r.script;
            if page <= 1 || s.served.is_none() {
                let i = s.next.min(s.replies.len() - 1);
                s.next += 1;
                s.served = Some(s.replies[i].clone());
            }
            Ok(s.served.clone().expect("a served reply"))
        })
    };
    let target = if req.query.is_empty() {
        path.clone()
    } else {
        format!("{path}?{}", req.query)
    };
    let etag = req.headers.get("if-none-match").map(String::as_str);
    let f = match routed {
        Some(Ok(reply)) => return Some(paged(shared, &path, &query, page, reply)),
        // 사용자 함수는 잠금 밖에서 불러요
        Some(Err(f)) => f,
        None => {
            if let Some(reply) = repo_content(shared, req, &path, &query) {
                return Some(reply);
            }
            shared.lock().status_with.clone()?
        }
    };
    Some(f(&target, etag))
}

fn value<'a>(query: &'a [(String, String)], name: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// `repositories/{id}/{끝}`을 원래 길로 돌려요. 모르는 id면 None(답하지 않음)이에요.
fn original_path(shared: &Shared, path: &str) -> Option<String> {
    let Some(rest) = path.strip_prefix("repositories/") else {
        return Some(path.to_string());
    };
    let (id, tail) = rest.split_once('/')?;
    let id: usize = id.parse().ok()?;
    let prefix = shared.lock().repo_ids.get(id.checked_sub(1)?)?.clone();
    Some(if prefix.is_empty() {
        tail.to_string()
    } else {
        format!("{prefix}/{tail}")
    })
}

/// 배열 답을 `per_page`대로 나누고 다음 쪽이 있으면 `Link`를 붙여요.
fn paged(
    shared: &Shared,
    path: &str,
    query: &[(String, String)],
    page: usize,
    reply: Reply,
) -> Reply {
    let (Reply::Json(Value::Array(items)), Some(per)) = (
        &reply,
        value(query, "per_page").and_then(|p| p.parse::<usize>().ok()),
    ) else {
        return reply;
    };
    let per = per.max(1);
    let start = (page.saturating_sub(1) * per).min(items.len());
    let end = (start + per).min(items.len());
    let body = Value::Array(items[start..end].to_vec())
        .to_string()
        .into_bytes();
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if end < items.len() {
        let parts: Vec<&str> = path.splitn(4, '/').collect();
        let (prefix, tail) = if parts.len() == 4 && parts[0] == "repos" {
            (parts[..3].join("/"), parts[3].to_string())
        } else {
            (String::new(), path.to_string())
        };
        let id = {
            let mut st = shared.lock();
            match st.repo_ids.iter().position(|p| *p == prefix) {
                Some(i) => i + 1,
                None => {
                    st.repo_ids.push(prefix);
                    st.repo_ids.len()
                }
            }
        };
        let kept: Vec<String> = query
            .iter()
            .filter(|(k, _)| k != "page" && k != "after")
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        let n = page + 1;
        let link = format!(
            "<{}repositories/{id}/{tail}?{}&after=c{n}&page={n}>; rel=\"next\"",
            shared.url,
            kept.join("&")
        );
        headers.push(("Link".to_string(), link));
    }
    Reply::Http {
        code: 200,
        headers,
        body,
    }
}

/// 길의 쿼리에서 쪽 값(`PAGING`)을 빼요. 맞출 때 보지 않아요.
fn without_paging(query: &[(String, String)]) -> Vec<(String, String)> {
    query
        .iter()
        .filter(|(k, _)| !PAGING.contains(&k.as_str()))
        .cloned()
        .collect()
}

/// 저장소 내용의 답이에요(모듈 설명의 표). 저장소 내용의 길이 아니면 None이에요.
fn repo_content(
    shared: &Shared,
    req: &Request,
    path: &str,
    query: &[(String, String)],
) -> Option<Reply> {
    let st = shared.lock();
    // GitHub처럼 저장소 이름은 대소문자를 가리지 않아요
    let (name, r) = st.repos.iter().find(|(name, _)| {
        let base = format!("repos/{name}");
        path.get(..base.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(&base))
            && matches!(path.as_bytes().get(base.len()), None | Some(b'/'))
    })?;
    let rest = path[format!("repos/{name}").len()..].trim_start_matches('/');
    if r.offline && (rest.starts_with("git/") || rest.starts_with("tarball/")) {
        return Some(Reply::Close);
    }
    let not_found = || http_reply(404, &[], Some(json!({"message": "Not Found"})));
    if rest.is_empty() {
        let short = name.rsplit('/').next().unwrap_or(name);
        return Some(Reply::Json(json!({
            "id": r.id,
            "name": short,
            "full_name": name,
            "default_branch": r.default_branch,
            "private": true,
        })));
    }
    if let Some(branch) = rest.strip_prefix("git/ref/heads/") {
        let branch = percent_decode_str(branch).decode_utf8_lossy().into_owned();
        return Some(match r.branches.get(&branch) {
            Some(sha) => Reply::Json(json!({
                "ref": format!("refs/heads/{branch}"),
                "object": {"sha": sha, "type": "commit"},
            })),
            None => not_found(),
        });
    }
    if let Some(sha) = rest.strip_prefix("git/trees/") {
        let root = r.commits.get(sha).map_or(sha, String::as_str);
        let Some(items) = r.trees.get(root) else {
            return Some(http_reply(
                422,
                &[],
                Some(
                    json!({"message": "Invalid object requested. SHA must identify a commit or a tree."}),
                ),
            ));
        };
        let recursive = value(query, "recursive").is_some();
        let mut out: Vec<Value> = Vec::new();
        if recursive {
            let mut todo = vec![(String::new(), items)];
            while let Some((prefix, items)) = todo.pop() {
                for i in items {
                    out.push(item_json(&format!("{prefix}{}", i.name), i));
                    if i.kind == "tree" {
                        todo.push((format!("{prefix}{}/", i.name), &r.trees[&i.sha]));
                    }
                }
            }
        } else {
            out = items.iter().map(|i| item_json(&i.name, i)).collect();
        }
        // 일부러 정렬하지 않은 순서예요(경로 역순): 받는 쪽이 정렬해요
        out.sort_by(|a, b| b["path"].as_str().cmp(&a["path"].as_str()));
        let truncated = recursive && r.truncate;
        if truncated {
            out.truncate(1);
        }
        return Some(Reply::Json(
            json!({"sha": root, "tree": out, "truncated": truncated}),
        ));
    }
    if let Some(sha) = rest.strip_prefix("git/blobs/") {
        let Some(bytes) = r.blobs.get(sha) else {
            return Some(not_found());
        };
        let raw = req.headers.get("accept").is_some_and(|a| a.contains("raw"));
        return Some(if raw {
            Reply::Http {
                code: 200,
                headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
                body: bytes.clone(),
            }
        } else {
            Reply::Json(json!({
                "sha": sha,
                "size": bytes.len(),
                "encoding": "base64",
                "content": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        });
    }
    if let Some(sha) = rest.strip_prefix("tarball/") {
        if !r.files.contains_key(sha) {
            return Some(not_found());
        }
        let location = format!("{}{name}/legacy.tar.gz/{sha}?token=x", shared.codeload);
        return Some(Reply::Http {
            code: 303,
            headers: vec![("Location".into(), location)],
            body: Vec::new(),
        });
    }
    None
}

fn item_json(path: &str, i: &Item) -> Value {
    json!({"path": path, "mode": i.mode, "type": i.kind, "sha": i.sha})
}

/// 두 번째 리스너(codeload)예요: `{owner}/{repo}/legacy.tar.gz/{sha}`에 tarball로 답해요.
fn serve_codeload(shared: &Shared, mut stream: TcpStream) {
    let Some(req) = read_request(&stream) else {
        return;
    };
    let built = {
        let mut st = shared.lock();
        st.codeload_served += 1;
        if req.headers.contains_key("authorization") {
            st.codeload_auth = true;
        }
        req.path
            .split_once("/legacy.tar.gz/")
            .and_then(|(name, sha)| {
                let r = st.repos.get(name)?;
                Some((tarball(name, sha, r)?, r.cut))
            })
    };
    let Some((body, cut)) = built else {
        write_reply(&mut stream, http_reply(404, &[], None));
        return;
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let end = if cut { body.len() / 2 } else { body.len() };
    let _ = stream.write_all(&body[..end]);
    let _ = stream.flush();
}

/// 커밋 `sha`의 tarball(gzip)이에요. 모르는 커밋이면 None이에요.
fn tarball(name: &str, sha: &str, r: &RepoState) -> Option<Vec<u8>> {
    use tar::{Builder, EntryType, Header};
    let files = r.files.get(sha)?;
    let top = format!("{}-{sha}", name.replace('/', "-"));
    let mut b = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    // pax 전역 머리(GitHub처럼 커밋 SHA를 적어요)
    let comment = format!("52 comment={sha}\n");
    let mut h = Header::new_ustar();
    h.set_entry_type(EntryType::XGlobalHeader);
    h.set_size(comment.len() as u64);
    h.set_mode(0o666);
    b.append_data(&mut h, "pax_global_header", comment.as_bytes())
        .ok()?;
    let dir = |b: &mut Builder<_>, path: &str| {
        let mut h = Header::new_gnu();
        h.set_entry_type(EntryType::Directory);
        h.set_size(0);
        h.set_mode(0o775);
        b.append_data(&mut h, format!("{path}/"), &[][..]).ok()
    };
    dir(&mut b, &top)?;
    let mut made: Vec<String> = Vec::new();
    for (path, mode, bytes) in files {
        let parts: Vec<&str> = path.split('/').collect();
        for n in 1..parts.len() {
            let folder = parts[..n].join("/");
            if !made.contains(&folder) {
                dir(&mut b, &format!("{top}/{folder}"))?;
                made.push(folder);
            }
        }
        let full = format!("{top}/{path}");
        let link = r
            .links
            .get(path)
            .map(String::as_str)
            .or_else(|| (mode == "120000").then(|| std::str::from_utf8(bytes).unwrap_or("")));
        if mode == "160000" {
            dir(&mut b, &full)?;
        } else if let Some(target) = link {
            let mut h = Header::new_gnu();
            h.set_entry_type(EntryType::Symlink);
            h.set_size(0);
            h.set_mode(0o777);
            b.append_link(&mut h, &full, target).ok()?;
        } else {
            let data = r.differs.get(path).unwrap_or(bytes);
            let mut h = Header::new_gnu();
            h.set_entry_type(EntryType::Regular);
            h.set_size(data.len() as u64);
            h.set_mode(if mode == "100755" { 0o775 } else { 0o664 });
            b.append_data(&mut h, &full, data.as_slice()).ok()?;
        }
    }
    for (raw, data) in &r.extra {
        // 이름을 머리에 그대로 써요(`append_data`는 `..`를 거절해요)
        let mut h = Header::new_old();
        let name = raw.as_bytes();
        h.as_old_mut().name[..name.len()].copy_from_slice(name);
        h.set_entry_type(EntryType::Regular);
        h.set_size(data.len() as u64);
        h.set_mode(0o664);
        h.set_cksum();
        b.append(&h, data.as_slice()).ok()?;
    }
    b.into_inner().ok()?.finish().ok()
}

/// 부르면 패닉하는 gh 토큰이에요: token·none 모드 테스트가 gh를 한 번도 묻지 않음을 지켜요.
#[derive(Debug, Default, Clone, Copy)]
pub struct PanicGhToken;

impl GhToken for PanicGhToken {
    fn token(&self) -> Result<String, SourceError> {
        panic!("PanicGhToken: gh was asked for a token outside gh mode");
    }
}

/// 차례로 답하는 가짜 gh 토큰이에요: `Some(토큰)`은 그 토큰, `None`은 gh가 없거나 로그아웃한 실패예요. 마지막
/// 답을 되풀이하고, 부른 수를 세요(`calls`).
#[derive(Debug)]
pub struct FakeGhToken {
    replies: Vec<Option<String>>,
    calls: Mutex<usize>,
}

impl FakeGhToken {
    pub fn new(replies: Vec<Option<&str>>) -> Self {
        assert!(!replies.is_empty(), "FakeGhToken needs at least one reply");
        FakeGhToken {
            replies: replies.into_iter().map(|r| r.map(str::to_string)).collect(),
            calls: Mutex::new(0),
        }
    }

    /// 늘 이 토큰이에요.
    pub fn gives(token: &str) -> Self {
        FakeGhToken::new(vec![Some(token)])
    }

    /// 늘 실패예요.
    pub fn fails() -> Self {
        FakeGhToken::new(vec![None])
    }

    pub fn calls(&self) -> usize {
        *self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl GhToken for FakeGhToken {
    fn token(&self) -> Result<String, SourceError> {
        let mut n = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        let reply = self.replies[(*n).min(self.replies.len() - 1)].clone();
        *n += 1;
        reply.ok_or_else(not_logged_in)
    }
}
