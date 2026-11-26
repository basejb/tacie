//! The playground for refresh tests. Every location is inside a temp folder: the real `data/`, `index/` and
//! `~/.config` are not touched. Neither git nor gh is launched.
//!
//! - GitHub is `FakeGitHub` (a loopback fake server, `gh`). The repos `o/old`, `o/new` and `o/other` live in it
//!   (`FakeRepo`, id 1, default branch `main`): `main` has README.md and app.py (both contain the repo name). The
//!   fake answers the head (`git/ref/heads/…`), tree, blob and tarball. `push` and `push_on` push a new commit that
//!   changes one file.
//! - Each repo has one issue (title `"{repo}의 이슈"`, body `"{repo}에서 받은 이슈예요."`) and an empty comment list, and the
//!   login is `basejb`. The change check (`sort=updated`) and other routes drop without an answer (a connection
//!   error).
//!   The connection is in gh mode and the gh token is always the fake's token (counts how many times `gh_token` was
//!   called).
//! - `data/sources/old`: the clone location of o/old made by the old path (only a `.git` folder). The engine no
//!   longer reads or deletes it.
//!   `data/sources/old-graph`: an empty wiring.json and a graph.json pointing at the first commit.
//! - `data/sources/github`: one saved copy of o/old (issue #1); `index/`: an index written with `START_META`.
//! - Embedding is `HashEmbed`, Slack is `FakeSlack` (one post in channel `#dev`).
//!
//! `fetch` and chunking are real, not fakes: the saved copy is exactly the items the real `fetch` would build from
//! the same list, so the first refresh does not rewrite that file and the chunks are the same.

#![allow(dead_code)] // 테스트마다 쓰는 도우미가 달라요

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tacie_sources::asana::AsanaSource;
use tacie_sources::atlassian::AtlassianSource;
use tacie_sources::dropbox::DropboxSource;
use tacie_sources::gdrive::GDriveSource;
use tacie_sources::linear::LinearSource;
use tacie_sources::linear::api::{LinearApi, Reply as LinearReply};
use tacie_sources::notion::{NotionApi, NotionSource};
use tacie_sources::testing::asana::FakeAsana;
use tacie_sources::testing::atlassian::FakeAtlassian;
use tacie_sources::testing::dropbox::FakeDropbox;
use tacie_sources::testing::gdrive::FakeDrive;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Map, Value, json};
use tacie_engine::pipeline::Engine;
use tacie_engine::refresh::{OpenIndex, Places, Refresher, Sources, Target};
use tacie_engine::server::{AppState, router};
use tacie_index::{Index, Mask, RawChunk, write_index};
use tacie_sources::SourceError;
use tacie_sources::gather::{Gather, gather};
use tacie_sources::github::content::{Entry, Snapshot, Tree, blob_sha};
use tacie_sources::github::gh_token::GhToken;
use tacie_sources::github::items::{blank_state, build_items, save_item, save_state};
use tacie_sources::github::{GitHub, Timeouts};
use tacie_sources::testing::FakeSlack;
use tacie_sources::testing::github::{Call, FakeGhToken, FakeGitHub, FakeRepo, Reply};
use tower::ServiceExt;

use super::fakes::{FakeAnswerer, FakeReranker, HashEmbed, services};
use super::fixture::engine_with;

/// 경로 → 바이트예요.
pub type Files = BTreeMap<String, Vec<u8>>;

/// 놀이터의 가짜 저장소들이에요.
pub const REPOS: [&str; 3] = ["o/old", "o/new", "o/other"];

/// 처음 색인의 meta예요.
pub fn start_meta() -> Map<String, Value> {
    let v = json!({"repo": "o/old", "sources": {
        "repo": {"path": "data/sources/old", "branch": "main", "graph": "data/sources/old-graph"},
        "github": {"repo": "o/old"}}});
    v.as_object().unwrap().clone()
}

/// 저장소의 처음 파일 둘이에요(둘 다 저장소 이름을 담아요).
pub fn first_files(name: &str) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "README.md".to_string(),
            format!("# {name}\n\n{name} 저장소의 설명이에요.\n").into_bytes(),
        ),
        (
            "app.py".to_string(),
            format!("NAME = '{name}'\n").into_bytes(),
        ),
    ])
}

/// 파일(경로 → 바이트, 모두 100644)로 만든 커밋 `commit`의 스냅숏이에요. 모든 파일의 바이트가 있어요.
pub fn snapshot_of(commit: &str, files: &BTreeMap<String, Vec<u8>>) -> Snapshot {
    let mut entries: Vec<Entry> = files
        .iter()
        .map(|(path, bytes)| Entry {
            path: path.clone(),
            mode: "100644".to_string(),
            sha: blob_sha(bytes),
        })
        .collect();
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    Snapshot {
        tree: Tree {
            commit: commit.to_string(),
            entries,
        },
        contents: files.clone().into_iter().collect(),
        graph: None,
    }
}

/// gh 모드의 gh 토큰이에요: 늘 가짜의 토큰을 주고(`FakeGhToken`), `panic`이 켜지면 패닉해요(갱신 스레드의 패닉
/// 시험), `fail`이 켜지면 gh가 토큰을 주지 못해요(key `gh is not logged in`). 토큰은 `GitHub`이 기억해서, 켠
/// 뒤에는 `set_credentials("gh", None)`으로 잊게 해요.
pub struct SandboxToken {
    pub inner: FakeGhToken,
    pub panic: AtomicBool,
    pub fail: AtomicBool,
}

impl SandboxToken {
    pub fn calls(&self) -> usize {
        self.inner.calls()
    }
}

impl GhToken for SandboxToken {
    fn token(&self) -> Result<String, SourceError> {
        if self.panic.load(Ordering::SeqCst) {
            panic!("SandboxToken panics as the test asked");
        }
        if self.fail.load(Ordering::SeqCst) {
            return FakeGhToken::fails().token();
        }
        self.inner.token()
    }
}

pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// 저장소의 이슈 하나예요(GitHub REST 목록의 모양).
pub fn issue(repo: &str, n: u64, title: &str, body: &str, updated: &str) -> Value {
    json!({"id": 1000 + n, "number": n, "title": title, "body": body, "user": {"login": "t"},
           "state": "open", "labels": [], "created_at": "2026-10-24T00:00:00Z", "updated_at": updated,
           "comments": 0, "html_url": format!("https://github.com/{repo}/issues/{n}")})
}

/// 저장소마다 있는 이슈 #1이에요.
pub fn first_issue(repo: &str) -> Value {
    issue(
        repo,
        1,
        &format!("{repo}의 이슈"),
        &format!("{repo}에서 받은 이슈예요."),
        "2026-10-24T00:00:00Z",
    )
}

/// 슬랙 원본이에요: 채널 `#dev`에 글 하나예요.
pub fn slack_raw() -> Value {
    json!({"team_url": "https://t.slack.com/", "users": {"U1": "민수"},
           "channels": [{"id": "C1", "name": "dev", "is_private": false, "members": null, "replies": {},
                         "messages": [{"ts": "1729503000.000400", "user": "U1", "text": "슬랙에서 받은 글이에요."}]}],
           "failed": [], "fetched_at": "2026-10-24T00:00:00Z"})
}

/// 폴더의 이름 → 수정 시각이에요. 폴더가 없으면 None이에요.
pub fn listing(dir: &Path) -> Option<BTreeMap<String, SystemTime>> {
    let entries = fs::read_dir(dir).ok()?;
    Some(
        entries
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().into_string().unwrap(),
                    e.metadata().unwrap().modified().unwrap(),
                )
            })
            .collect(),
    )
}

/// 폴더 아래 모든 파일의 상대 경로 → 바이트예요.
pub fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                out.insert(rel, fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// 수정 시각을 옛날로 돌려요: 다시 쓰면 바뀌어요.
pub fn age(path: &Path) {
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

/// 코드·문서 조각만이에요.
pub fn code_of(raw: &[RawChunk]) -> Vec<RawChunk> {
    raw.iter()
        .filter(|c| matches!(c["src"].as_str(), Some("code" | "docs")))
        .cloned()
        .collect()
}

/// 상태가 `running`이 아닐 때까지 30 s까지 기다려요.
pub fn finish(r: &Refresher) -> Value {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let st = r.status();
        if st["state"] != "running" {
            return (*st).clone();
        }
        assert!(Instant::now() < end, "still running: {st}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `{"repo"?, "branch"?, "fresh"?}`예요.
pub fn target(repo: Option<&str>, branch: Option<&str>, fresh: bool) -> Target {
    Target {
        repo: repo.map(str::to_string),
        branch: branch.map(str::to_string),
        fresh,
    }
}

/// 색인을 열기 직전에 부르는 함수예요: 지우기 중에 상태와 `start`를
/// 보는 데 써요.
pub type OpenHook = Arc<Mutex<Option<Box<dyn FnMut() + Send>>>>;

/// 서버 요청의 토큰이에요.
pub const TOKEN: &str = "t0k";

pub struct Sandbox {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    /// GitHub REST 가짜예요.
    pub gh: Arc<FakeGitHub>,
    /// gh 모드의 토큰이에요(늘 가짜의 토큰).
    pub gh_token: Arc<SandboxToken>,
    pub github: Arc<GitHub>,
    pub slack: Arc<FakeSlack>,
    /// 노션이에요: 연결하지 않으면(`notion.set`) 받지 않아요. 페이지는 `notion_api.pages`예요.
    pub notion: Arc<NotionSource>,
    pub notion_api: Arc<FakeNotion>,
    /// Linear예요: 연결하지 않으면(`linear.set`) 받지 않아요. 이슈는 `linear_api.issues`예요.
    pub linear: Arc<LinearSource>,
    pub linear_api: Arc<FakeLinear>,
    /// Google Drive예요: 연결하지 않으면(`gdrive.set`) 받지 않아요. 파일은 `gdrive_api`예요.
    pub gdrive: Arc<GDriveSource>,
    pub gdrive_api: Arc<FakeDrive>,
    /// Jira·Confluence예요: 연결하지 않으면(`atlassian.set`) 받지 않아요. 글은 `atlassian_api`예요.
    pub atlassian: Arc<AtlassianSource>,
    pub atlassian_api: Arc<FakeAtlassian>,
    /// Asana예요: 연결하지 않으면(`asana.set`) 받지 않아요. 글은 `asana_api`예요(시계는 만들 때의 지금이에요).
    pub asana: Arc<AsanaSource>,
    pub asana_api: Arc<FakeAsana>,
    /// Dropbox예요: 연결하지 않으면(`dropbox.set`) 받지 않아요. 파일은 `dropbox_api`예요(계정은 `dbid:A`예요).
    pub dropbox: Arc<DropboxSource>,
    pub dropbox_api: Arc<FakeDropbox>,
    pub embed: Arc<HashEmbed>,
    pub engine: Arc<Engine>,
    pub r: Arc<Refresher>,
    pub open_hook: OpenHook,
    /// (저장소, 브랜치) → 그 브랜치 머리의 파일이에요.
    trees: Mutex<HashMap<(String, String), Files>>,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        Sandbox::with_timeouts(None)
    }

    /// 요청 시간 제한을 바꾼 놀이터예요(`GitHub::with_timeouts`).
    pub fn with_timeouts(timeouts: Option<Timeouts>) -> Sandbox {
        Sandbox::build(timeouts, None)
    }

    /// `min`개 이상 임베딩하는 동기화를 `every`개씩 나눠 검색에 넣는 놀이터예요(`Refresher::with_partial`).
    pub fn with_partial(min: usize, every: usize) -> Sandbox {
        Sandbox::build(None, Some((min, every)))
    }

    fn build(timeouts: Option<Timeouts>, partial: Option<(usize, usize)>) -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();

        let gh = FakeGitHub::start();
        // 가짜 저장소가 없는 이름도 저장소 자체는 있어요(id 1). 그 밖의 길(바뀜 확인 포함)은 오프라인이에요
        gh.status_with(|path, _| {
            let repo = path.strip_prefix("repos/").unwrap_or("");
            if !repo.contains('?')
                && repo.split('/').count() == 2
                && repo.split('/').all(|p| !p.is_empty())
            {
                Reply::Json(json!({"id": 1, "full_name": repo}))
            } else {
                Reply::Close
            }
        });
        gh.login(Reply::Json(json!({"login": "basejb"})));
        let mut trees = HashMap::new();
        let mut first = String::new();
        for name in REPOS {
            let files = first_files(name);
            let commit = push_files(&gh.repo(name, 1), "main", &files);
            if name == "o/old" {
                first = commit;
            }
            trees.insert((name.to_string(), "main".to_string()), files);
        }
        let gh_token = Arc::new(SandboxToken {
            inner: FakeGhToken::gives(FakeGitHub::TOKEN),
            panic: AtomicBool::new(false),
            fail: AtomicBool::new(false),
        });
        let mut github = GitHub::new("gh", &gh.url(), gh_token.clone())
            .unwrap()
            .with_pause(|_| {});
        if let Some(t) = timeouts {
            github = github.with_timeouts(t);
        }
        let github = Arc::new(github);
        let slack = FakeSlack::writes(slack_raw());
        let notion_api = Arc::new(FakeNotion::default());
        let notion = Arc::new(NotionSource::with_api(notion_api.clone()));
        let linear_api = Arc::new(FakeLinear::new());
        let linear = Arc::new(LinearSource::with_api(linear_api.clone()));
        let gdrive_api = Arc::new(FakeDrive::new());
        let gdrive = Arc::new(GDriveSource::with_api(gdrive_api.clone()));
        let atlassian_api = Arc::new(FakeAtlassian::new());
        let atlassian = Arc::new(AtlassianSource::with_api(atlassian_api.clone()));
        let asana_api = Arc::new(FakeAsana::new());
        asana_api.set_now(chrono::Utc::now());
        let asana = Arc::new(AsanaSource::with_api(asana_api.clone()));
        let dropbox_api = Arc::new(FakeDropbox::new());
        let dropbox = Arc::new(DropboxSource::with_api(dropbox_api.clone()));

        let sources = root.join("data/sources");
        // 옛 길이 만든 클론 자리예요: 엔진은 이제 읽지도 지우지도 않아요
        fs::create_dir_all(sources.join("old/.git")).unwrap();
        fs::write(sources.join("old/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let graph = sources.join("old-graph");
        fs::create_dir_all(&graph).unwrap();
        fs::write(graph.join("wiring.json"), r#"{"nodes": []}"#).unwrap();
        fs::write(
            graph.join("graph.json"),
            json!({"commit": first}).to_string(),
        )
        .unwrap();
        // o/old의 저장본이에요: 진짜 fetch가 같은 목록에서 만들 항목 그대로예요
        let raw = sources.join("github");
        for (_, item) in build_items(&[first_issue("o/old")], &[], &[], &BTreeMap::new()) {
            save_item(&raw, &item).unwrap();
        }
        let mut state = blank_state();
        state.insert("repo".into(), json!("o/old"));
        state.insert("fetched_at".into(), json!("2026-10-24T00:00:00Z"));
        save_state(&raw, &state).unwrap();

        for repo in REPOS {
            Self::lists_on(&gh, repo, vec![first_issue(repo)]);
        }

        let wiring = graph.join("wiring.json");
        let snap = snapshot_of(&first, &first_files("o/old"));
        let (chunks, _) = gather(
            &github,
            slack.as_ref(),
            Gather {
                snapshot: Some(&snap),
                graph: Some(&wiring),
                skip: &[],
                github_repo: Some("o/old"),
                github_raw: &raw,
                slack_raw: None,
                fetch: false,
                code: None,
                cached: None,
            },
            &mut Vec::new(),
        )
        .unwrap();
        let embed = Arc::new(HashEmbed::default());
        let index_dir = root.join("index");
        {
            let e = embed.clone();
            write_index(
                &index_dir,
                &chunks,
                &mut |t| tacie_embed::Embed::embed(e.as_ref(), t, Some(512)),
                &start_meta(),
                None,
                None,
            )
            .unwrap();
        }
        embed.texts.lock().unwrap().clear();

        let open_hook: OpenHook = Arc::new(Mutex::new(None));
        let hook = open_hook.clone();
        let open_index: OpenIndex = Arc::new(move |d: &Path| {
            // 시험이 일부러 패닉하게 한 함수도 있어서 독이 들어도 써요
            if let Some(f) = hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_mut()
            {
                f();
            }
            Index::open(d, Mask::Plans, &HashSet::new())
        });
        let (mut engine, _) = engine_with(
            open_index(&index_dir).unwrap(),
            services(
                Arc::new(FakeReranker::all(0.9)),
                Arc::new(FakeAnswerer::ok("")),
            ),
        );
        engine.embedder = embed.clone();
        let engine = Arc::new(engine);
        let r = Refresher::new(
            engine.clone(),
            index_dir,
            Places::new(&root.join("data")),
            Sources {
                github: github.clone(),
                slack: slack.clone(),
                notion: notion.clone(),
                linear: linear.clone(),
                gdrive: gdrive.clone(),
                atlassian: atlassian.clone(),
                asana: asana.clone(),
                dropbox: dropbox.clone(),
            },
            open_index,
        )
        .with_status_log();
        let r = Arc::new(match partial {
            Some((min, every)) => r.with_partial(min, every),
            None => r,
        });
        gh.clear_calls();
        Sandbox {
            _dir: dir,
            root,
            gh,
            gh_token,
            github,
            slack,
            notion,
            notion_api,
            linear,
            linear_api,
            gdrive,
            gdrive_api,
            atlassian,
            atlassian_api,
            asana,
            asana_api,
            dropbox,
            dropbox_api,
            embed,
            engine,
            r,
            open_hook,
            trees: Mutex::new(trees),
        }
    }

    /// 색인을 열기 직전에 부를 함수를 두거나(Some) 치워요(None).
    pub fn on_open(&self, f: Option<Box<dyn FnMut() + Send>>) {
        *self
            .open_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = f;
    }

    /// 이 놀이터의 엔진, GitHub, 갱신기로 띄운 서버 상태예요.
    pub fn app(&self) -> Arc<AppState> {
        Arc::new(AppState {
            engine: self.engine.clone(),
            token: TOKEN.to_string(),
            github: self.github.clone(),
            refresher: Some(self.r.clone()),
            meeting_dir: None,
        })
    }

    /// `router()`에 요청 하나를 넣어요(포트를 열지 않아요). 토큰을 붙이고 본문은 그대로예요. (상태 코드, JSON 본문)
    pub fn call(&self, method: &str, path: &str, body: &str) -> (StatusCode, Value) {
        self.call_with(method, path, body, Some(&format!("Bearer {TOKEN}")))
    }

    pub fn call_with(
        &self,
        method: &str,
        path: &str,
        body: &str,
        auth: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(a) = auth {
            req = req.header(header::AUTHORIZATION, a);
        }
        let req = req.body(Body::from(body.to_string())).unwrap();
        let app = router(self.app());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let res = app.oneshot(req).await.unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice(&bytes).unwrap())
        })
    }

    /// 저장소의 세 목록이에요: 이 이슈들과 빈 댓글·리뷰 댓글 목록이에요. 바뀜 확인(`sort=updated`)은 목록이 아니라
    /// 오프라인이에요(더 자세한 길이 이겨요).
    fn lists_on(fake: &FakeGitHub, repo: &str, issues: Vec<Value>) {
        for probe in [
            "issues?state=all&sort=updated",
            "issues/comments?sort=updated",
            "pulls/comments?sort=updated",
        ] {
            fake.route(&format!("repos/{repo}/{probe}"), Reply::Close);
        }
        fake.route(
            &format!("repos/{repo}/issues?state=all"),
            Reply::Json(Value::Array(issues)),
        );
        fake.route(
            &format!("repos/{repo}/issues/comments"),
            Reply::Json(json!([])),
        );
        fake.route(
            &format!("repos/{repo}/pulls/comments"),
            Reply::Json(json!([])),
        );
    }

    pub fn lists(&self, repo: &str, issues: Vec<Value>) {
        Self::lists_on(&self.gh, repo, issues);
    }

    /// o/old의 GitHub 저장본 폴더예요(`data/sources/github`).
    pub fn raw(&self) -> PathBuf {
        self.root.join("data/sources/github")
    }

    pub fn index_dir(&self) -> PathBuf {
        self.root.join("index")
    }

    /// 가짜 저장소예요.
    pub fn repo(&self, name: &str) -> FakeRepo {
        self.gh.repo(name, 1)
    }

    /// `repo`의 `branch`(없으면 `main`의 지금 파일에서 만들어요)에 파일 하나를 바꾼 커밋을 올려요.
    /// 커밋 SHA예요.
    pub fn push_on(&self, repo: &str, branch: &str, file: &str, content: &str) -> String {
        let mut trees = self.trees.lock().unwrap();
        let main = trees[&(repo.to_string(), "main".to_string())].clone();
        let files = trees
            .entry((repo.to_string(), branch.to_string()))
            .or_insert(main);
        files.insert(file.to_string(), content.as_bytes().to_vec());
        push_files(&self.repo(repo), branch, files)
    }

    pub fn push(&self, repo: &str, file: &str, content: &str) -> String {
        self.push_on(repo, "main", file, content)
    }

    /// `repo`의 `branch` 머리의 파일이에요.
    pub fn files_of(&self, repo: &str, branch: &str) -> BTreeMap<String, Vec<u8>> {
        self.trees.lock().unwrap()[&(repo.to_string(), branch.to_string())].clone()
    }

    /// o/old의 코드 받기(머리, 트리, blob, tarball)가 답 없이 끊겨요. 저장소 확인과 목록은 그대로예요.
    pub fn code_offline(&self) {
        self.repo("o/old").offline(true);
    }

    /// GitHub이 o/old에 닿지 못해요: 저장소 확인이 오프라인이에요.
    pub fn github_offline(&self) {
        self.gh.status_seq("repos/o/old", vec![Reply::Close]);
    }

    pub fn github_back(&self) {
        self.gh.status(
            "repos/o/old",
            200,
            &[],
            Some(json!({"id": 1, "full_name": "o/old"})),
        );
    }

    /// 갱신 스레드가 GitHub에 물으려고 토큰을 얻는 자리에서 패닉해요(켜면 기억한 토큰을 잊게 해요).
    pub fn panic_on_token(&self, on: bool) {
        self.gh_token.panic.store(on, Ordering::SeqCst);
        if on {
            self.github.set_credentials("gh", None);
        }
    }

    /// gh가 토큰을 주지 못해요(gh 로그아웃, 없는 gh). 켜면 기억한 토큰을 잊게 해요.
    pub fn gh_token_fails(&self, on: bool) {
        self.gh_token.fail.store(on, Ordering::SeqCst);
        if on {
            self.github.set_credentials("gh", None);
        }
    }

    /// 받은 요청이에요.
    pub fn calls(&self) -> Vec<Call> {
        self.gh.calls()
    }

    /// 코드 받기의 요청 경로예요(`repos/{r}/` 뒤): `git/ref/heads/…`, `git/trees/…`, `git/blobs/…`, `tarball/…`.
    pub fn code_calls(&self) -> Vec<String> {
        self.gh
            .calls()
            .iter()
            .filter_map(|c| {
                let rest = c.path.strip_prefix("repos/")?;
                let mut parts = rest.splitn(3, '/');
                let (_, _, tail) = (parts.next()?, parts.next()?, parts.next()?);
                (tail.starts_with("git/") || tail.starts_with("tarball/")).then(|| tail.to_string())
            })
            .collect()
    }

    /// 지금 색인의 모든 조각 글이에요.
    pub fn texts(&self) -> String {
        self.engine
            .index
            .load()
            .raw
            .iter()
            .map(|c| c["text"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn meta(&self) -> Map<String, Value> {
        self.engine.index.load().meta.clone()
    }

    /// 메모리의 색인 meta만 바꿔요.
    pub fn set_meta(&self, meta: Map<String, Value>) {
        let ix = self.engine.index.load();
        self.engine.index.store(Arc::new(ix.with_meta(meta)));
    }

    /// 갱신을 하나 해서 끝난 상태를 돌려줘요.
    pub fn run(&self, t: Target) -> Value {
        assert!(self.r.start(t), "a refresh is already running");
        finish(&self.r)
    }

    /// 갱신이 `done`으로 끝나야 해요. 임베딩한 수예요.
    pub fn sync(&self, t: Target) -> u64 {
        let st = self.run(t);
        assert_eq!(st["state"], "done", "{st}");
        st["embedded"].as_u64().unwrap()
    }
}

/// 파일 전체(모두 100644)로 `branch`에 새 커밋을 올려요. 커밋 SHA예요.
fn push_files(repo: &FakeRepo, branch: &str, files: &BTreeMap<String, Vec<u8>>) -> String {
    let list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), "100644", b.as_slice()))
        .collect();
    repo.push(branch, &list)
}

/// 가짜 노션이에요: `pages`의 (id, 제목, 본문)마다 페이지 하나, 본문은 문단 하나예요.
#[derive(Default)]
pub struct FakeNotion {
    pub pages: Mutex<Vec<(String, String, String)>>,
    /// 참이면 모든 호출이 실패해요(down).
    pub fail: AtomicBool,
}

impl NotionApi for FakeNotion {
    fn call(&self, _method: &str, path: &str, _body: Option<&Value>) -> Result<Value, SourceError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(SourceError::service(
                tacie_sources::Kind::Down,
                "notion: down",
            ));
        }
        let pages = self.pages.lock().unwrap().clone();
        if path == "search" {
            let results: Vec<Value> = pages
                .iter()
                .map(|(id, title, _)| {
                    json!({"object": "page", "id": id, "url": format!("https://www.notion.so/{id}"),
                           "last_edited_time": "2026-01-01T00:00:00.000Z", "parent": {"type": "workspace"},
                           "properties": {"Name": {"type": "title", "title": [{"plain_text": title}]}}})
                })
                .collect();
            return Ok(json!({"results": results, "has_more": false}));
        }
        let id = path
            .trim_start_matches("blocks/")
            .split('/')
            .next()
            .unwrap_or("");
        let body = pages
            .iter()
            .find(|p| p.0 == id)
            .map(|p| p.2.clone())
            .unwrap_or_default();
        Ok(
            json!({"results": [{"id": format!("{id}-b"), "type": "paragraph", "has_children": false,
                               "paragraph": {"rich_text": [{"plain_text": body}]}}], "has_more": false}),
        )
    }
}

/// 가짜 Linear예요: `teams`(처음엔 t1, t2)와 `issues`만 답하고 나머지 목록은 비어요. `refuse`면 거절(key),
/// `busy`면 한도(busy)예요. `gate`에 (들어왔음, 놓아줌) 짝을 넣으면 다음 이슈 받기가 들어왔다고 알리고 놓아줄 때까지
/// 기다려요(갱신 중에 연결을 바꾸는 테스트). `queries`는 받은 질의 수예요.
pub struct FakeLinear {
    pub teams: Mutex<Vec<Value>>,
    pub issues: Mutex<Vec<Value>>,
    pub refuse: AtomicBool,
    pub busy: AtomicBool,
    /// 참이면 이슈는 답하고 그다음 목록(문서)부터 한도예요(처음 받기가 중간에 끊긴 경우).
    pub busy_after_issues: AtomicBool,
    pub gate: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    pub queries: std::sync::atomic::AtomicUsize,
}

impl FakeLinear {
    pub fn new() -> FakeLinear {
        FakeLinear {
            teams: Mutex::new(vec![
                json!({"id": "t1", "name": "Core", "key": "COR"}),
                json!({"id": "t2", "name": "Web", "key": "WEB"}),
            ]),
            issues: Mutex::new(Vec::new()),
            refuse: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            busy_after_issues: AtomicBool::new(false),
            gate: Mutex::new(None),
            queries: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// 이슈 하나를 더해요.
    pub fn issue(&self, id: &str, team: &str, title: &str, body: &str, updated: &str) {
        self.issues.lock().unwrap().push(json!({
            "id": id, "identifier": format!("{}-{id}", team.to_uppercase()), "title": title, "description": body,
            "url": format!("https://linear.app/x/issue/{id}"), "state": {"name": "Todo", "type": "unstarted"},
            "team": {"id": team, "name": team}, "createdAt": updated, "updatedAt": updated, "trashed": false}));
    }
}

impl Default for FakeLinear {
    fn default() -> Self {
        FakeLinear::new()
    }
}

impl LinearApi for FakeLinear {
    fn query(&self, q: &str, vars: &Value) -> Result<LinearReply, SourceError> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        if self.refuse.load(Ordering::SeqCst) {
            return Err(SourceError::service(
                tacie_sources::Kind::Key,
                "linear: refused",
            ));
        }
        if self.busy.load(Ordering::SeqCst) {
            return Err(SourceError::service(
                tacie_sources::Kind::Busy,
                "linear: rate limited",
            ));
        }
        let name = q
            .trim_start()
            .strip_prefix("query ")
            .unwrap_or("")
            .split(['(', ' ', '{'])
            .next()
            .unwrap_or("")
            .to_string();
        if self.busy_after_issues.load(Ordering::SeqCst) && name == "LinearDocuments" {
            return Err(SourceError::service(
                tacie_sources::Kind::Busy,
                "linear: rate limited",
            ));
        }
        let empty = json!({"nodes": [], "pageInfo": {"hasNextPage": false}});
        let mut issues: Vec<Value> = self.issues.lock().unwrap().clone();
        issues.sort_by(|a, b| a["updatedAt"].as_str().cmp(&b["updatedAt"].as_str()));
        let data = match name.as_str() {
            "LinearTeams" => {
                json!({"teams": {"nodes": self.teams.lock().unwrap().clone(), "pageInfo": {"hasNextPage": false}}})
            }
            "LinearIssues" | "LinearIssueIds" => {
                if name == "LinearIssues" {
                    let gate = self.gate.lock().unwrap().take();
                    if let Some((entered, release)) = gate {
                        let _ = entered.send(());
                        let _ = release.recv();
                    }
                }
                let since = vars["since"].as_str().unwrap_or("");
                let nodes: Vec<Value> = issues
                    .into_iter()
                    .filter(|i| i["updatedAt"].as_str().unwrap_or("") > since)
                    .collect();
                json!({"issues": {"nodes": nodes, "pageInfo": {"hasNextPage": false}}})
            }
            "LinearCheck" => {
                let newest: Vec<Value> = issues.last().cloned().into_iter().collect();
                json!({"issues": {"nodes": newest}, "documents": {"nodes": []}, "comments": {"nodes": []}, "projectUpdates": {"nodes": []}})
            }
            "LinearDocuments" | "LinearDocumentIds" => json!({"documents": empty}),
            "LinearUpdates" | "LinearUpdateIds" => json!({"projectUpdates": empty}),
            _ => json!({"comments": empty}),
        };
        Ok(LinearReply {
            data: Some(data),
            errors: vec![],
        })
    }
}
