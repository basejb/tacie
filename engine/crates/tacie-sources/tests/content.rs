//! Fetching repo content (spec section 5, `github::content`): the branch head commit, trees (path byte order, walking
//! truncated trees), the blobs API and one tarball pinned to the commit, and blob SHA checks. GitHub is answered by
//! the repo content of `FakeGitHub` (`FakeRepo`).

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use tacie_sources::error::Kind;
use tacie_sources::github::GitHub;
use tacie_sources::github::content::{
    self, BLOB_WORKERS, TARBALL_MIN, TARBALL_SHARE, blob_sha, download, head_commit, snapshot,
    tree, use_tarball,
};
use tacie_sources::testing::github::{Call, FakeGitHub, FakeRepo, PanicGhToken, Reply};

const REPO: &str = "o/n";

/// 가짜의 토큰을 가진 token 모드 연결이에요.
fn token(fake: &FakeGitHub) -> GitHub {
    let gh = GitHub::new("none", &fake.url(), Arc::new(PanicGhToken))
        .unwrap()
        .with_pause(|_| {});
    gh.set_credentials("token", Some(FakeGitHub::TOKEN));
    gh
}

fn setup() -> (Arc<FakeGitHub>, FakeRepo, GitHub) {
    let fake = FakeGitHub::start();
    let repo = fake.repo(REPO, 7);
    let gh = token(&fake);
    (fake, repo, gh)
}

/// `n`개의 서로 다른 파일이에요(`f00.py`…).
fn many(n: usize) -> Vec<(String, Vec<u8>)> {
    (0..n)
        .map(|i| (format!("f{i:02}.py"), format!("print({i})\n").into_bytes()))
        .collect()
}

fn push_all(repo: &FakeRepo, files: &[(String, Vec<u8>)]) -> String {
    let list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), "100644", b.as_slice()))
        .collect();
    repo.push("main", &list)
}

/// 이 경로들의 (경로, blob SHA)예요(트리에서 읽어요).
fn wants(gh: &GitHub, commit: &str, paths: &[&str]) -> Vec<(String, String)> {
    let t = tree(gh, REPO, commit).unwrap().unwrap();
    let blobs = t.blobs();
    paths
        .iter()
        .map(|p| (p.to_string(), blobs[*p].clone()))
        .collect()
}

fn count(calls: &[Call], part: &str) -> usize {
    calls.iter().filter(|c| c.path.contains(part)).count()
}

#[test]
fn blob_sha_is_gits() {
    assert_eq!(blob_sha(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    assert_eq!(
        blob_sha(b"hello\n"),
        "ce013625030ba8dba906f756967f9e9ca394464a"
    );
}

#[test]
fn constants_match_the_spec() {
    assert_eq!(BLOB_WORKERS, 4);
    assert_eq!(TARBALL_MIN, 40);
    assert_eq!(TARBALL_SHARE, 4);
}

#[test]
fn tree_entries_sort_like_ls_files() {
    let (fake, repo, gh) = setup();
    // 가짜는 트리 항목을 일부러 거꾸로 줘요
    let paths = ["a0", "한글.md", "a/b", "_x", "a.b", "B", "a-b", "a/c/d"];
    let files: Vec<(&str, &str, &[u8])> =
        paths.iter().map(|p| (*p, "100644", p.as_bytes())).collect();
    let commit = repo.push("main", &files);
    let t = tree(&gh, REPO, &commit).unwrap().unwrap();
    assert_eq!(t.commit, commit);
    let mut expected: Vec<&str> = paths.to_vec();
    expected.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    assert_eq!(
        expected,
        ["B", "_x", "a-b", "a.b", "a/b", "a/c/d", "a0", "한글.md"]
    );
    assert_eq!(t.files(), expected);
    let listed: Vec<&str> = t.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(listed, expected);
    assert_eq!(t.blobs()["a/b"], blob_sha(b"a/b"));
    let calls = fake.calls();
    let c = calls
        .iter()
        .find(|c| c.path == format!("repos/o/n/git/trees/{commit}"))
        .unwrap();
    assert_eq!(c.query, "recursive=1");
    assert!(c.has_token);
}

#[test]
fn files_blobs_and_links_split_by_mode() {
    let (_fake, repo, gh) = setup();
    let commit = repo.push(
        "main",
        &[
            ("a.py", "100644", b"x = 1\n"),
            ("run.sh", "100755", b"#!/bin/sh\n"),
            ("link", "120000", b"a.py"),
            ("vendor/lib", "160000", b"sub"),
        ],
    );
    let t = tree(&gh, REPO, &commit).unwrap().unwrap();
    assert_eq!(t.files(), ["a.py", "run.sh"]);
    let blobs = t.blobs();
    let keys: Vec<&str> = blobs.keys().map(String::as_str).collect();
    assert_eq!(keys, ["a.py", "link", "run.sh"]);
    assert_eq!(blobs["link"], blob_sha(b"a.py"));
    assert_eq!(t.links(), HashSet::from(["link".to_string()]));
    let modes: Vec<&str> = t.entries.iter().map(|e| e.mode.as_str()).collect();
    assert_eq!(modes, ["100644", "120000", "100755"]);
}

#[test]
fn head_commit_reads_the_exact_branch() {
    let (fake, repo, gh) = setup();
    let commit = repo.push("feat/x", &[("a.py", "100644", b"1")]);
    assert_eq!(head_commit(&gh, REPO, "feat/x", None).unwrap(), commit);
    // 저장소는 보이는데 그 브랜치가 없으면(지운 브랜치) down이에요: 다시 연결할 일이 아니에요
    let e = head_commit(&gh, REPO, "feat", None).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
    assert_eq!(e.detail(), content::BRANCH_GONE);
    let paths: Vec<String> = fake.calls().into_iter().map(|c| c.path).collect();
    assert_eq!(
        paths,
        [
            "repos/o/n/git/ref/heads/feat/x",
            "repos/o/n/git/ref/heads/feat",
            "repos/o/n"
        ]
    );
}

#[test]
fn head_commit_of_a_hidden_repo_is_key() {
    let (fake, _repo, gh) = setup();
    // 저장소가 없거나 볼 수 없으면(둘 다 404) 처음 오류 그대로 key예요
    let not_found = || Reply::Http {
        code: 404,
        headers: vec![],
        body: br#"{"message": "Not Found"}"#.to_vec(),
    };
    fake.route("repos/o/gone/git/ref/heads/main", not_found());
    fake.route("repos/o/gone", not_found());
    let e = head_commit(&gh, "o/gone", "main", None).unwrap_err();
    assert_eq!(
        (e.kind(), e.detail()),
        (Kind::Key, "GitHub answered HTTP 404".to_string())
    );
    let paths: Vec<String> = fake.calls().into_iter().map(|c| c.path).collect();
    assert_eq!(paths, ["repos/o/gone/git/ref/heads/main", "repos/o/gone"]);
    // 저장소 답이 404가 아닌 실패여도(권한 거절) 처음 오류예요
    fake.route(
        "repos/o/gone",
        Reply::Http {
            code: 403,
            headers: vec![],
            body: br#"{"message": "Resource not accessible by integration"}"#.to_vec(),
        },
    );
    let e = head_commit(&gh, "o/gone", "main", None).unwrap_err();
    assert_eq!(
        (e.kind(), e.detail()),
        (Kind::Key, "GitHub answered HTTP 404".to_string())
    );
    // 브랜치 답이 404가 아니면 저장소에 묻지 않아요
    let before = fake.calls().len();
    fake.route(
        "repos/o/gone/git/ref/heads/dev",
        Reply::Http {
            code: 401,
            headers: vec![],
            body: br#"{"message": "Bad credentials"}"#.to_vec(),
        },
    );
    assert_eq!(
        head_commit(&gh, "o/gone", "dev", None).unwrap_err().kind(),
        Kind::Key
    );
    assert_eq!(fake.calls().len(), before + 1);
}

#[test]
fn head_commit_needs_a_full_sha() {
    let (fake, _repo, gh) = setup();
    for sha in ["abc", "ZZ5f2a1c3b8e9d0f7a6b4c2e1d3f5a7b9c0e2d4f", ""] {
        fake.route(
            "repos/o/n/git/ref/heads/main",
            Reply::Json(serde_json::json!({"object": {"sha": sha, "type": "commit"}})),
        );
        let e = head_commit(&gh, REPO, "main", None).unwrap_err();
        assert_eq!(e.kind(), Kind::Down, "{sha}");
    }
}

#[test]
fn branch_names_are_encoded_in_the_path() {
    let (fake, repo, gh) = setup();
    let commit = repo.push("feat/한 #1", &[("a.py", "100644", b"1")]);
    assert_eq!(head_commit(&gh, REPO, "feat/한 #1", None).unwrap(), commit);
    let calls = fake.calls();
    assert_eq!(
        calls[0].path,
        "repos/o/n/git/ref/heads/feat/%ED%95%9C%20%231"
    );
    assert_eq!(calls[0].query, "");
}

#[test]
fn missing_commit_tree_is_none() {
    let (fake, _repo, gh) = setup();
    let none = "0123456789abcdef0123456789abcdef01234567";
    // 가짜는 모르는 커밋에 422예요(진짜 GitHub처럼)
    assert!(tree(&gh, REPO, none).unwrap().is_none());
    fake.status(&format!("repos/o/n/git/trees/{none}"), 404, &[], None);
    assert!(tree(&gh, REPO, none).unwrap().is_none());
    // 트리가 없는 커밋의 스냅숏은 down이에요(key가 아니에요: 갱신은 그것을 받기 실패로 봐요)
    let e = snapshot(&gh, REPO, none, None, &[], None).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
    assert_eq!(e.detail(), "GitHub does not have the commit to index");
    fake.status(&format!("repos/o/n/git/trees/{none}"), 500, &[], None);
    let e = tree(&gh, REPO, none).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
    fake.status(&format!("repos/o/n/git/trees/{none}"), 401, &[], None);
    assert_eq!(tree(&gh, REPO, none).unwrap_err().kind(), Kind::Key);
}

#[test]
fn truncated_tree_walks_subtrees() {
    let (fake, repo, gh) = setup();
    let commit = repo.push(
        "main",
        &[
            ("README.md", "100644", b"hi"),
            ("src/a.rs", "100644", b"fn a() {}"),
            ("src/lib/b.rs", "100755", b"fn b() {}"),
            ("docs/c.md", "100644", b"# c"),
            ("docs/link", "120000", b"c.md"),
        ],
    );
    let whole = tree(&gh, REPO, &commit).unwrap().unwrap();
    repo.truncate_trees(true);
    fake.clear_calls();
    let walked = tree(&gh, REPO, &commit).unwrap().unwrap();
    assert_eq!(walked.commit, whole.commit);
    let pairs = |t: &content::Tree| -> Vec<(String, String, String)> {
        t.entries
            .iter()
            .map(|e| (e.path.clone(), e.mode.clone(), e.sha.clone()))
            .collect()
    };
    assert_eq!(pairs(&walked), pairs(&whole));
    assert_eq!(
        walked.files(),
        ["README.md", "docs/c.md", "src/a.rs", "src/lib/b.rs"]
    );
    // 폴더 4개(맨 위, docs, src, src/lib) + 처음의 recursive 요청 1개
    let calls = fake.calls();
    assert_eq!(count(&calls, "/git/trees/"), 5);
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.query.contains("recursive"))
            .count(),
        1
    );
}

#[test]
fn a_truncated_folder_is_down_not_a_silent_cut() {
    let (fake, repo, gh) = setup();
    let commit = repo.push("main", &[("a.py", "100644", b"1")]);
    // 맨 위가 recursive 없이도 잘려 와요: 더 나눌 수 없어요
    fake.route(
        &format!("repos/o/n/git/trees/{commit}"),
        Reply::Json(serde_json::json!({"sha": commit, "tree": [], "truncated": true})),
    );
    let e = tree(&gh, REPO, &commit).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
    assert_eq!(e.detail(), "GitHub's tree is too large");
}

#[test]
fn branch_segments_that_leave_the_path_are_refused() {
    let (fake, _repo, gh) = setup();
    for bad in ["", "../x", "a/../b", "a//b", ".hidden", "a/.b", "a/"] {
        let e = head_commit(&gh, REPO, bad, None).unwrap_err();
        assert_eq!(e.kind(), Kind::Key, "{bad:?}");
    }
    assert!(fake.calls().is_empty());
}

#[test]
fn few_missing_use_blobs() {
    let (fake, repo, gh) = setup();
    let files = many(10);
    let commit = push_all(&repo, &files);
    let want = wants(&gh, &commit, &["f01.py", "f04.py", "f09.py"]);
    fake.clear_calls();
    let got = download(&gh, REPO, &commit, &want, 10).unwrap();
    assert_eq!(got.len(), 3);
    assert_eq!(got["f04.py"], b"print(4)\n");
    let calls = fake.calls();
    assert_eq!(count(&calls, "/git/blobs/"), 3);
    assert_eq!(count(&calls, "/tarball/"), 0);
    for c in calls.iter().filter(|c| c.path.contains("/git/blobs/")) {
        assert_eq!(c.accept.as_deref(), Some("application/vnd.github.raw+json"));
        assert!(c.has_token);
    }
}

#[test]
fn same_blob_is_asked_once() {
    let (fake, repo, gh) = setup();
    let commit = repo.push(
        "main",
        &[("a.py", "100644", b"same"), ("b.py", "100644", b"same")],
    );
    let want = wants(&gh, &commit, &["a.py", "b.py"]);
    fake.clear_calls();
    let got = download(&gh, REPO, &commit, &want, 2).unwrap();
    assert_eq!(got["a.py"], b"same");
    assert_eq!(got["b.py"], b"same");
    assert_eq!(count(&fake.calls(), "/git/blobs/"), 1);
}

#[test]
fn nothing_wanted_asks_nothing() {
    let (fake, _repo, gh) = setup();
    let got = download(
        &gh,
        REPO,
        "0123456789abcdef0123456789abcdef01234567",
        &[],
        0,
    )
    .unwrap();
    assert!(got.is_empty());
    assert!(fake.calls().is_empty());
}

#[test]
fn many_missing_use_one_tarball() {
    let (fake, repo, gh) = setup();
    let files = many(60);
    let mut list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), "100644", b.as_slice()))
        .collect();
    // 링크와 서브모듈은 tarball에 링크·폴더로 있어요
    list.push(("link.py", "120000", b"f00.py"));
    list.push(("sub", "160000", b"x"));
    list.push(("깊은/폴더/한글 이름.md", "100644", b"# \xed\x95\x9c\n"));
    let commit = repo.push("main", &list);
    let mut paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    paths.push("깊은/폴더/한글 이름.md");
    let want = wants(&gh, &commit, &paths);
    fake.clear_calls();
    let got = download(&gh, REPO, &commit, &want, 61).unwrap();
    assert_eq!(got.len(), 61);
    for (p, b) in &files {
        assert_eq!(&got[p], b, "{p}");
    }
    assert_eq!(got["깊은/폴더/한글 이름.md"], b"# \xed\x95\x9c\n");
    let calls = fake.calls();
    assert_eq!(count(&calls, "/git/blobs/"), 0);
    let tarballs: Vec<&Call> = calls
        .iter()
        .filter(|c| c.path.contains("/tarball/"))
        .collect();
    assert_eq!(tarballs.len(), 1);
    assert_eq!(tarballs[0].path, format!("repos/o/n/tarball/{commit}"));
    assert!(tarballs[0].has_token);
    assert!(fake.codeload_served() >= 1);
    assert!(!fake.codeload_saw_authorization());
}

#[test]
fn use_tarball_boundaries() {
    assert!(!use_tarball(40, 100));
    assert!(use_tarball(41, 100));
    assert!(!use_tarball(100, 1000));
    assert!(use_tarball(251, 1000));
    assert!(!use_tarball(0, 0));
    assert!(use_tarball(41, 0));
}

#[test]
fn tarball_mismatch_falls_back_to_blobs() {
    let (fake, repo, gh) = setup();
    let files = many(60);
    let commit = push_all(&repo, &files);
    repo.tarball_differs("f07.py", b"print('$Format:%H$')\n");
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    let want = wants(&gh, &commit, &paths);
    fake.clear_calls();
    let got = download(&gh, REPO, &commit, &want, 60).unwrap();
    assert_eq!(got["f07.py"], b"print(7)\n");
    let calls = fake.calls();
    assert_eq!(count(&calls, "/tarball/"), 1);
    let blobs: Vec<&Call> = calls
        .iter()
        .filter(|c| c.path.contains("/git/blobs/"))
        .collect();
    assert_eq!(blobs.len(), 1);
    assert_eq!(
        blobs[0].path,
        format!("repos/o/n/git/blobs/{}", blob_sha(b"print(7)\n"))
    );
}

#[test]
fn tarball_entries_outside_the_top_folder_are_ignored() {
    let (fake, repo, gh) = setup();
    let files = many(60);
    let commit = push_all(&repo, &files);
    let top = format!("o-n-{commit}");
    let right = b"print(1)\n";
    // 진짜 자리의 f01.py는 어긋나고, 맞는 바이트는 이상한 자리에만 있어요: 하나라도 받아들이면 blobs를 묻지 않아요
    repo.tarball_differs("f01.py", b"not this\n");
    for raw in [
        "../f01.py".to_string(),
        "/f01.py".to_string(),
        "f01.py".to_string(),
        format!("{top}/../f01.py"),
        format!("{top}/./f01.py"),
        format!("{top}//f01.py"),
        format!("/{top}/f01.py"),
    ] {
        repo.tarball_extra(&raw, right);
    }
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    let want = wants(&gh, &commit, &paths);
    fake.clear_calls();
    let got = download(&gh, REPO, &commit, &want, 60).unwrap();
    assert_eq!(got["f01.py"], right);
    assert_eq!(got.len(), 60);
    let calls = fake.calls();
    assert_eq!(count(&calls, "/tarball/"), 1);
    assert_eq!(count(&calls, "/git/blobs/"), 1);
}

#[test]
fn tarball_links_are_not_read() {
    let (fake, repo, gh) = setup();
    let files = many(60);
    let mut list: Vec<(&str, &str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), "100644", b.as_slice()))
        .collect();
    // 트리에서는 보통 파일이지만 tarball에서는 링크인 항목: 링크는 읽지 않고 blobs로 받아요
    list.push(("odd.py", "100644", b"f00.py"));
    let commit = repo.push("main", &list);
    repo.tarball_link("odd.py", "f00.py");
    let mut paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    paths.push("odd.py");
    let want = wants(&gh, &commit, &paths);
    fake.clear_calls();
    let got = download(&gh, REPO, &commit, &want, 61).unwrap();
    assert_eq!(got["odd.py"], b"f00.py");
    assert_eq!(count(&fake.calls(), "/git/blobs/"), 1);
}

#[test]
fn blob_with_the_wrong_sha_fails() {
    let (fake, repo, gh) = setup();
    let commit = repo.push("main", &[("a.py", "100644", b"x = 1\n")]);
    let want = wants(&gh, &commit, &["a.py"]);
    fake.route(
        &format!("repos/o/n/git/blobs/{}", want[0].1),
        Reply::Http {
            code: 200,
            headers: vec![],
            body: b"x = 2\n".to_vec(),
        },
    );
    let e = download(&gh, REPO, &commit, &want, 1).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
    assert_eq!(
        e.detail(),
        "GitHub sent a file that does not match its tree"
    );
}

#[test]
fn blob_errors_keep_their_kind() {
    let (fake, repo, gh) = setup();
    let commit = repo.push("main", &[("a.py", "100644", b"x = 1\n")]);
    let want = wants(&gh, &commit, &["a.py"]);
    let path = format!("repos/o/n/git/blobs/{}", want[0].1);
    fake.status(&path, 404, &[], None);
    assert_eq!(
        download(&gh, REPO, &commit, &want, 1).unwrap_err().kind(),
        Kind::Key
    );
    fake.status(&path, 429, &[], None);
    assert_eq!(
        download(&gh, REPO, &commit, &want, 1).unwrap_err().kind(),
        Kind::Busy
    );
}

#[test]
fn download_failure_is_network() {
    let (fake, repo, gh) = setup();
    let files = many(60);
    let commit = push_all(&repo, &files);
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    let all = wants(&gh, &commit, &paths);
    // blobs 하나가 끊겨요
    fake.route(&format!("repos/o/n/git/blobs/{}", all[2].1), Reply::Close);
    let e = download(&gh, REPO, &commit, &all[..3], 60).unwrap_err();
    assert_eq!(e.kind(), Kind::Network);
    // tarball 요청이 끊겨요
    fake.route(&format!("repos/o/n/tarball/{commit}"), Reply::Close);
    let e = download(&gh, REPO, &commit, &all, 60).unwrap_err();
    assert_eq!(e.kind(), Kind::Network);
}

#[test]
fn a_cut_tarball_is_network_without_the_codeload_address() {
    let (fake, repo, gh) = setup();
    let files = many(60);
    let commit = push_all(&repo, &files);
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    let want = wants(&gh, &commit, &paths);
    repo.codeload_cut(true);
    let e = download(&gh, REPO, &commit, &want, 60).unwrap_err();
    assert_eq!(e.kind(), Kind::Network);
    let text = format!("{} {e:?}", e.detail());
    assert!(!text.contains("token"), "{text}");
    assert!(!text.contains("codeload"), "{text}");
    assert!(!text.contains("127.0.0.1"), "{text}");
    assert!(!text.contains(REPO), "{text}");
    assert_eq!(count(&fake.calls(), "/git/blobs/"), 0);
}

#[test]
fn tarball_errors_keep_their_kind() {
    let (fake, repo, gh) = setup();
    let files = many(60);
    let commit = push_all(&repo, &files);
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    let want = wants(&gh, &commit, &paths);
    fake.status(&format!("repos/o/n/tarball/{commit}"), 404, &[], None);
    let e = download(&gh, REPO, &commit, &want, 60).unwrap_err();
    assert_eq!(e.kind(), Kind::Key);
    assert_eq!(e.detail(), "GitHub answered HTTP 404");
    // gzip이 아닌 몸통은 down이에요
    fake.route(
        &format!("repos/o/n/tarball/{commit}"),
        Reply::Http {
            code: 200,
            headers: vec![],
            body: b"not a tarball".to_vec(),
        },
    );
    let e = download(&gh, REPO, &commit, &want, 60).unwrap_err();
    assert_eq!(e.kind(), Kind::Down);
}

#[test]
fn fake_repo_answers_the_repository() {
    let (fake, repo, gh) = setup();
    repo.default_branch("dev");
    let (status, _, body) = gh.status("repos/o/n", &[], None).unwrap();
    assert_eq!(status, 200);
    let body = body.unwrap();
    assert_eq!(body["id"], 7);
    assert_eq!(body["default_branch"], "dev");
    assert_eq!(body["full_name"], "o/n");
    // 브랜치를 다시 밀면 머리가 바뀌고 옛 커밋의 트리도 남아요
    let one = repo.push("dev", &[("a.py", "100644", b"1")]);
    let two = repo.push("dev", &[("a.py", "100644", b"2")]);
    assert_ne!(one, two);
    assert_eq!(head_commit(&gh, REPO, "dev", None).unwrap(), two);
    let old: BTreeMap<String, String> = tree(&gh, REPO, &one).unwrap().unwrap().blobs();
    assert_eq!(old["a.py"], blob_sha(b"1"));
    // 저장소 이름은 대소문자를 가리지 않아요(GitHub처럼)
    assert_eq!(head_commit(&gh, "O/N", "dev", None).unwrap(), two);
    // 오프라인이면 내용 요청만 끊겨요
    repo.offline(true);
    assert_eq!(
        head_commit(&gh, REPO, "dev", None).unwrap_err().kind(),
        Kind::Network
    );
    assert_eq!(gh.status("repos/o/n", &[], None).unwrap().0, 200);
    repo.offline(false);
    assert_eq!(head_commit(&gh, REPO, "dev", None).unwrap(), two);
    assert!(fake.unanswered().is_empty());
}
