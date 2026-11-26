"""The engine's HTTP contract for /index, /index/refresh|check|clear|rebuild and /github/*, on a temp root
(conftest.sources_engine): an empty index and a fake GitHub server (fake_github.py: o/n's main is the two files of
work/, one issue), with a gh that records its calls and answers `auth token`, and a git that records its calls and
fails, both first on PATH. No test uses a real git or gh.

All tests share one engine. `built` (the first refresh) gives every test that needs it a built index, so each test
also runs alone (-k). Responses are compared with their key order (`exactly`).
"""
import json
import re
import shutil
import time
from types import SimpleNamespace

import pytest

from .conftest import README_MARK, start_sources_engine
from .fake_github import GH_TOKEN, ISSUE, ISSUE_MARK, LOGIN, TOKEN
from .test_core import call

INDEX_KEYS = ["chunks", "commit", "built_at", "refresh", "files", "github_items", "bytes", "repo", "branch",
              "symbols_missing", "github_lost", "github_login", "connected", "cleared", "notion", "linear", "gdrive",
              "atlassian", "asana", "dropbox", "notes", "model_ready",
              "voice_ready", "sample"]
DONE_KEYS = ["state", "finished_at", "chunks", "embedded", "commit", "warnings"]
GITHUB_CHUNK_KEYS = ["src", "path", "title", "author", "at", "acl", "line", "url", "text"]
TEXTS = ["file: app.py\nNAME = 'o/n'",
         f"doc: README.md\nsection: o/n\n# o/n\n\no/n 저장소의 설명이에요. {README_MARK}",
         f"github: 이슈 #1 · 연결 확인\nmeta: 열림 · sora-p · 2026-10-01\n설명이에요. {ISSUE_MARK}"]
NOT_AVAILABLE = "GitHub is not available"
REPOS = "user/repos?affiliation=owner,collaborator,organization_member&sort=pushed"
BAD_TARGET = {"error": "send {repo?: owner/name, branch?: a branch name, fresh?: true or false}"}
APP_NEXT = "NAME = 'o/n'\nVERSION = 2\n"
WAIT_S = 120


def exactly(got, status, want):
    """`got` (status, JSON) is `status` and `want`, keys in the same order at every level."""
    assert (got[0], json.dumps(got[1], ensure_ascii=False)) == (status, json.dumps(want, ensure_ascii=False))


def wait_refresh(e):
    """GET /index until the refresh (or clear) is over; its `refresh` status."""
    deadline = time.monotonic() + WAIT_S
    while time.monotonic() < deadline:
        status, got = call(e.url + "/index")
        assert status == 200
        if got["refresh"]["state"] not in ("running", "clearing"):
            return got["refresh"]
        time.sleep(0.2)
    raise AssertionError(f"the refresh did not end in {WAIT_S} s: {got['refresh']}")


def read(e, name):
    return json.loads((e.root / "index" / name).read_text(encoding="utf-8"))


@pytest.fixture(scope="module")
def built(sources_engine):
    """The first refresh of the empty index: /index before it, the /index/refresh answer, its done status, the fake's
    head it built."""
    e = sources_engine
    status, first = call(e.url + "/index")
    assert status == 200
    started = call(e.url + "/index/refresh", "POST")
    done = wait_refresh(e)
    assert done["state"] == "done", done
    return SimpleNamespace(first=first, started=started, done=done, head=e.head())


def test_refresh_check_clear_rebuild_flow(sources_engine, built):
    e = sources_engine
    first = built.first
    assert list(first) == INDEX_KEYS
    assert (first["chunks"], first["refresh"], first["github_login"], first["connected"], first["cleared"]) == (
        0, {"state": "idle"}, LOGIN, True, False)
    status, got = built.started
    assert (status, list(got), got["started"]) == (200, ["started", "refresh"], True)
    done, head = built.done, built.head
    assert list(done) == DONE_KEYS, done
    assert (done["state"], done["chunks"], done["embedded"], done["commit"], done["warnings"]) == (
        "done", 3, 3, head[:8], [])

    chunks = read(e, "chunks.json")
    assert [c["path"] for c in chunks] == ["app.py", "README.md", "github/issue/1"]
    assert [c["text"] for c in chunks] == TEXTS
    assert list(chunks[2]) == GITHUB_CHUNK_KEYS
    assert (chunks[2]["author"], chunks[2]["at"], chunks[2]["url"]) == ("sora-p", "2026-10-01", ISSUE["html_url"])
    meta = read(e, "meta.json")
    assert list(meta) == ["repo", "sources", "commit", "built_at", "embed"]
    fetched_at = meta["sources"]["github"]["fetched_at"]
    assert isinstance(fetched_at, str) and fetched_at
    assert json.dumps(meta["sources"], ensure_ascii=False) == json.dumps({
        "repo": {"path": "data/sources/o__n", "branch": "main", "symbols_missing": 0},
        "github": {"repo": "o/n", "fetched_at": fetched_at, "chunks": 1, "raw": "data/sources/github"}},
        ensure_ascii=False)  # with key order
    assert (meta["repo"], meta["commit"], meta["embed"]) == ("o/n", head, {"model": "BAAI/bge-m3", "tokens": 512})
    assert (e.root / "data/sources/github/items/issue-1.json").is_file()  # under the temp root, not the repo's data/
    status, got = call(e.url + "/index")
    assert (got["chunks"], got["commit"], got["files"], got["github_items"], got["repo"], got["branch"]) == (
        3, head, 2, 1, "o/n", "main")

    # nothing new: the index stays, nothing is embedded
    status, got = call(e.url + "/index/refresh", "POST")
    assert (status, list(got), got["started"]) == (200, ["started", "refresh"], True)
    done = wait_refresh(e)
    assert list(done) == [*DONE_KEYS, "unchanged"], done
    assert (done["state"], done["chunks"], done["embedded"], done["unchanged"]) == ("done", 3, 0, True)

    exactly(call(e.url + "/index/check", "POST"), 200, {"code": False, "github": False, "changed": False})
    e.push({"app.py": APP_NEXT})
    assert e.head() != head
    before = len(e.api_calls())
    exactly(call(e.url + "/index/check", "POST"), 200, {"code": True, "github": False, "changed": True})
    # the second check sent the saved ETags: the fake answered 304 to every probe
    probes = {c["path"]: c["if_none_match"] for c in e.api_calls()[before:] if "per_page=1" in c["query"]}
    assert probes == {"repos/o/n/issues": '"e-issues"', "repos/o/n/issues/comments": '"e-comments"',
                      "repos/o/n/pulls/comments": '"e-review-comments"'}, probes
    assert "repos/o/n/git/ref/heads/main" in [c["path"] for c in e.api_calls()[before:]]

    exactly(call(e.url + "/index/clear", "POST"), 200, {"cleared": True})
    assert not (e.root / "data/sources/github").exists()
    # no clone is made, and another repo's folder is left alone
    assert not (e.root / "data/sources/repos/o__n").exists() and not (e.root / "data/sources/o__n").exists()
    assert (e.root / "data/sources/repos/x__y/keep").read_text() == "keep\n"
    status, got = call(e.url + "/index")
    assert (got["chunks"], got["commit"], got["cleared"], got["refresh"]["state"]) == (0, None, True, "idle")

    status, got = call(e.url + "/index/rebuild", "POST", {"fresh": True})
    assert (status, list(got), got["started"]) == (200, ["started", "refresh"], True)
    done = wait_refresh(e)
    assert done["state"] == "done", done
    status, got = call(e.url + "/index")
    new_head = e.head()
    assert (got["chunks"], got["cleared"], got["files"], got["github_items"], got["commit"]) == (
        3, False, 2, 1, new_head)


def test_rebuild_rejects_bad_targets(sources_engine):
    e = sources_engine
    _, before = call(e.url + "/index")
    for body in ({"repo": "o/n\n"}, {"repo": "nope"}, {"branch": "-x"}, {"branch": "@{-1}"}, {"fresh": "yes"},
                 {"repo": 1}, []):
        exactly(call(e.url + "/index/rebuild", "POST", body), 400, BAD_TARGET)
    exactly(call(e.url + "/index/rebuild", "POST", data=b"{"), 400, {"error": "body is not JSON"})
    _, after = call(e.url + "/index")
    assert after["refresh"] == before["refresh"]  # nothing started


def test_github_listing_routes(sources_engine):
    e = sources_engine
    old = {"full_name": "o/old", "private": False, "default_branch": "dev", "pushed_at": "2026-09-01T00:00:00Z",
           "id": 7}
    new = {"full_name": "o/n", "private": True, "default_branch": "main", "pushed_at": "2026-10-20T00:00:00Z"}
    e.set_routes({
        REPOS: {"body": [old, new]},
        "user/orgs": {"body": [{"login": "acme"}, {"login": "open"}, {"login": "sso"}, {"login": "bad org"}]},
        "orgs/acme/repos?type=private&per_page=1": {"status": 403, "body": {
            "message": "Although you appear to have the correct authorization credentials, the `acme` organization "
                       "has enabled OAuth App access restrictions, meaning that data access to third-parties is "
                       "limited."}},
        "orgs/open/repos?type=private&per_page=1": {"body": []},
        "orgs/sso/repos?type=private&per_page=1": {"status": 403, "body": {
            "message": "Resource protected by organization SAML enforcement. You must grant your OAuth token access "
                       "to this organization."}},
        "repos/o/n": {"body": {"id": 1, "full_name": "o/n", "default_branch": "main"}},
        "repos/o/n/branches": {"body": [{"name": "dev"}, {"name": "main"}]},
    })
    try:
        exactly(call(e.url + "/github/repos"), 200, {
            "repos": [{"full_name": "o/n", "private": True, "default_branch": "main",
                       "pushed_at": "2026-10-20T00:00:00Z"},
                      {"full_name": "o/old", "private": False, "default_branch": "dev",
                       "pushed_at": "2026-09-01T00:00:00Z"}],
            "needs_approval": ["acme"], "needs_sso": ["sso"]})
        exactly(call(e.url + "/github/branches?repo=o/n"), 200, {"branches": ["main", "dev"], "default": "main"})
        for query in ("?repo=o/n%0A", "?repo=", "?repo=nope", "?repo=o/..", "?repo=-x/y", ""):
            exactly(call(e.url + "/github/branches" + query), 400, {"error": "repo must be owner/name"})
    finally:
        e.set_routes({})


def test_github_failures_are_502_with_a_kind(sources_engine):
    e = sources_engine
    cases = [({"status": 401, "body": {"message": "Bad credentials"}}, "key"),
             ({"close": True}, "network"),
             ({"status": 403, "headers": {"X-RateLimit-Remaining": "0"},
               "body": {"message": "API rate limit exceeded for user ID 1."}}, "busy"),
             ({"status": 429, "body": {"message": "slow down"}}, "busy"),
             ({"status": 502, "body": {"message": "Server Error"}}, "down")]
    try:
        for route, kind in cases:
            e.set_routes({REPOS: route, "repos/o/n": route})
            for path in ("/github/repos", "/github/branches?repo=o/n"):
                got = call(e.url + path)
                exactly(got, 502, {"error": NOT_AVAILABLE, "error_kind": kind})
                assert "127.0.0.1" not in json.dumps(got[1]) and "message" not in json.dumps(got[1])
        e.set_routes({REPOS: {"raw": '[[{"full_name": "o/'}, "repos/o/n": {"raw": '{"default_branch'}})
        for path in ("/github/repos", "/github/branches?repo=o/n"):  # a cut-off answer is "down"
            exactly(call(e.url + path), 502, {"error": NOT_AVAILABLE, "error_kind": "down"})
    finally:
        e.set_routes({})


def test_credentials_and_no_connection(sources_engine, built):
    e = sources_engine
    api_before, gh_before = len(e.api_calls()), len(e.gh_calls())
    try:
        exactly(call(e.url + "/github/credentials", "POST", {"mode": "token", "token": TOKEN}), 200,
                {"connected": True, "login": LOGIN})
        assert [(c["method"], c["path"], c["has_token"]) for c in e.api_calls()[api_before:]] == [
            ("GET", "user", True)]  # login again, with the token
        status, got = call(e.url + "/index")
        assert (status, got["connected"], got["github_login"]) == (200, True, LOGIN)
        assert TOKEN not in json.dumps(got)

        # with the token, everything that reaches GitHub: a refresh, a check and the repo list
        status, got = call(e.url + "/index/refresh", "POST")
        assert (status, got["started"]) == (200, True)
        done = wait_refresh(e)
        # the code comes over the API with the token too: nothing new, nothing missing
        assert (done["state"], done["chunks"], done["embedded"], done["warnings"], done.get("unchanged")) == (
            "done", 3, 0, [], True), done
        exactly(call(e.url + "/index/check", "POST"), 200, {"code": False, "github": False, "changed": False})
        e.set_routes({REPOS: {"body": []}, "user/orgs": {"body": []}})
        exactly(call(e.url + "/github/repos"), 200, {"repos": [], "needs_approval": [], "needs_sso": []})
        e.set_routes({})
        with_token = e.api_calls()[api_before:]
        assert len(with_token) > 5 and all(c["has_token"] for c in with_token), with_token
        paths = [c["path"] for c in with_token]
        assert "repos/o/n/git/ref/heads/main" in paths and any(p.startswith("repos/o/n/git/trees/") for p in paths)
        assert e.gh_calls()[gh_before:] == []  # token mode asks gh nothing
    finally:
        e.set_routes({})
        api_mark, gh_mark = len(e.api_calls()), len(e.gh_calls())
        exactly(call(e.url + "/github/credentials", "POST", {"mode": "gh"}), 200,
                {"connected": True, "login": LOGIN})
    # gh mode: gh is asked for its token once for the new connection, and the fake lets that token in
    assert e.gh_calls()[gh_mark:] == [["auth", "token"]]
    assert [(c["path"], c["has_token"]) for c in e.api_calls()[api_mark:]] == [("user", True)]
    exactly(call(e.url + "/index/check", "POST"), 200, {"code": False, "github": False, "changed": False})
    assert e.gh_calls()[gh_mark:] == [["auth", "token"]]  # the token is kept for the connection
    assert all(c["has_token"] for c in e.api_calls()[api_mark:])
    assert not any(TOKEN in ln or GH_TOKEN in ln for ln in e.stderr())

    try:
        exactly(call(e.url + "/github/credentials", "POST", {"mode": "none"}), 200,
                {"connected": False, "login": None})
        api, gh = len(e.api_calls()), len(e.gh_calls())
        status, got = call(e.url + "/index/refresh", "POST")
        assert (status, got["started"]) == (200, True)
        done = wait_refresh(e)
        assert (done["state"], done["error"], done["error_kind"]) == ("failed", "GitHub is not connected", "key")
        status, got = call(e.url + "/index")
        assert (got["connected"], got["github_login"], got["chunks"]) == (False, None, 3)  # the last index answers
        exactly(call(e.url + "/index/check", "POST"), 200, {"code": None, "github": None, "changed": False})
        assert (len(e.api_calls()), len(e.gh_calls())) == (api, gh)  # nothing borrowed the user's logins
    finally:
        gh_mark = len(e.gh_calls())
        exactly(call(e.url + "/github/credentials", "POST", {"mode": "gh"}), 200,
                {"connected": True, "login": LOGIN})
    assert e.gh_calls()[gh_mark:] == [["auth", "token"]]
    assert e.git_calls() == []


# what each engine prints as a refresh ends (engine/refresh.py, tacie-engine refresh.rs): proof its lines reach stderr.
# Since 2026-10-27 the Rust line ends with the time the sync took (", took 6.8 s").
DONE_LINE = re.compile(r"^refresh: done, 3 chunks, \d+ embedded, commit [0-9a-f]{8}(, took \d+\.\d s)?$", re.M)
REPO_NAME = re.compile(r"(?<![\w/.-])o/n(?![\w/.-])")


def test_logs_carry_no_content(sources_engine, built):
    e = sources_engine
    err = "\n".join(e.stderr())
    assert DONE_LINE.search(err), err[-2000:]
    fake_at = e.github.url.removeprefix("http://").rstrip("/")
    for text in (ISSUE_MARK, README_MARK, "연결 확인", "설명이에요", ISSUE["user"]["login"], TOKEN, GH_TOKEN,
                 f"{fake_at}/repos", e.github.codeload_url):
        assert text not in err, text
    assert not REPO_NAME.search(err), REPO_NAME.search(err)
    log = (e.root / "github.log").read_text(encoding="utf-8")
    for text in (ISSUE_MARK, README_MARK, "연결 확인", "설명이에요", TOKEN, GH_TOKEN, "token="):
        assert text not in log, text  # the fake's own log: paths and flags, no content and no token
        assert not any(text in " ".join(c) for c in e.gh_calls() + e.git_calls()), text
    assert e.git_calls() == []


def test_engine_starts_without_an_index(fakes, tmp_path_factory):
    """With no index/ the engine starts on an empty seed (cleared, no repo), and /index/rebuild {repo, branch} builds
    the first repository."""

    def no_index(root):
        shutil.rmtree(root / "index")

    with start_sources_engine(fakes, tmp_path_factory, prepare=no_index, mode="gh") as e:
        assert any("started empty" in ln for ln in e.stderr())
        assert read(e, "chunks.json") == []
        assert list(read(e, "meta.json")) == ["repo", "commit", "built_at", "cleared_at"]
        status, got = call(e.url + "/index")
        assert (status, got["chunks"], got["repo"], got["branch"], got["cleared"], got["refresh"]) == (
            200, 0, None, None, True, {"state": "idle"})

        status, got = call(e.url + "/index/refresh", "POST")
        assert (status, got["started"]) == (200, True)
        failed = wait_refresh(e)
        assert (failed["state"], failed["error"]) == (
            "failed", "no repository yet: choose one with /index/rebuild {repo, branch}"), failed

        status, got = call(e.url + "/index/rebuild", "POST", {"repo": "o/n", "branch": "main"})
        assert (status, got["started"]) == (200, True)
        done = wait_refresh(e)
        assert (done["state"], done["chunks"], done["warnings"]) == ("done", 3, []), done
        assert [c["text"] for c in read(e, "chunks.json")] == TEXTS
        meta = read(e, "meta.json")
        assert "cleared_at" not in meta and meta["repo"] == "o/n"
        assert meta["sources"]["repo"]["path"] == "data/sources/repos/o__n"
        status, got = call(e.url + "/index")
        assert (got["chunks"], got["repo"], got["branch"], got["cleared"]) == (3, "o/n", "main", False)
        assert not (e.root / "data/sources/repos/o__n").exists()  # nothing is cloned
        assert e.git_calls() == []


def test_no_process_in_token_mode(fakes, tmp_path_factory):
    """Started with no connection and then given a token, no route runs gh or git (spec 7-4)."""
    with start_sources_engine(fakes, tmp_path_factory, mode="none") as e:
        status, got = call(e.url + "/index")
        assert (status, got["connected"]) == (200, False)
        exactly(call(e.url + "/github/credentials", "POST", {"mode": "token", "token": TOKEN}), 200,
                {"connected": True, "login": LOGIN})
        status, got = call(e.url + "/index/rebuild", "POST", {})
        assert (status, got["started"]) == (200, True)
        done = wait_refresh(e)
        assert (done["state"], done["chunks"], done["warnings"]) == ("done", 3, []), done
        status, got = call(e.url + "/index/refresh", "POST")
        assert (status, got["started"]) == (200, True)
        done = wait_refresh(e)
        assert (done["state"], done.get("unchanged")) == ("done", True), done
        exactly(call(e.url + "/index/check", "POST"), 200, {"code": False, "github": False, "changed": False})
        e.set_routes({REPOS: {"body": []}, "user/orgs": {"body": []},
                      "repos/o/n/branches": {"body": [{"name": "main"}]}})
        exactly(call(e.url + "/github/repos"), 200, {"repos": [], "needs_approval": [], "needs_sso": []})
        exactly(call(e.url + "/github/branches?repo=o/n"), 200, {"branches": ["main"], "default": "main"})
        exactly(call(e.url + "/index/clear", "POST"), 200, {"cleared": True})
        assert wait_refresh(e)["state"] == "idle"
        assert all(c["has_token"] for c in e.api_calls())
        assert (e.gh_calls(), e.git_calls()) == ([], [])


def test_gh_mode_only_when_the_launcher_asks(fakes, tmp_path_factory):
    """Started without TACIE_GITHUB_MODE the engine has no connection, never runs gh, and refuses gh mode (400): only
    the dev app's launch (TACIE_GITHUB_MODE=gh) may use the gh login."""
    with start_sources_engine(fakes, tmp_path_factory, mode=None) as e:
        status, got = call(e.url + "/index")
        assert (status, got["connected"], got["github_login"]) == (200, False, None)
        exactly(call(e.url + "/github/credentials", "POST", {"mode": "gh"}), 400,
                {"error": "gh mode is off: the engine was not started with TACIE_GITHUB_MODE=gh"})
        status, got = call(e.url + "/index/refresh", "POST")
        assert (status, got["started"]) == (200, True)
        done = wait_refresh(e)
        assert (done["state"], done["error_kind"]) == ("failed", "key"), done
        status, got = call(e.url + "/index")
        assert got["connected"] is False
        assert (e.gh_calls(), e.api_calls()) == ([], [])


def test_many_new_files_come_in_one_tarball(fakes, tmp_path_factory):
    """A first sync that misses more than TARBALL_MIN (40) files takes the commit's tarball: the API answers 303 to
    codeload, and codeload never sees the token."""
    with start_sources_engine(fakes, tmp_path_factory, mode="gh") as e:
        e.push({f"pkg/m{i:02d}.py": f"VALUE_{i} = {i}\n" for i in range(45)})
        status, got = call(e.url + "/index/rebuild", "POST", {"repo": "o/n", "branch": "main"})
        assert (status, got["started"]) == (200, True)
        done = wait_refresh(e)
        assert (done["state"], done["warnings"]) == ("done", []), done
        head = e.head()
        paths = [c["path"] for c in e.api_calls()]
        assert f"repos/o/n/tarball/{head}" in paths, paths
        assert not any(p.startswith("repos/o/n/git/blobs/") for p in paths), paths
        codeload = e.codeload_calls()
        assert [c["path"] for c in codeload] == [f"o/n/legacy.tar.gz/{head}"], codeload
        assert all(c["authorization_on_codeload"] is False for c in codeload)
        texts = [c["text"] for c in read(e, "chunks.json")]
        assert any("VALUE_44 = 44" in t for t in texts)



def test_notion_credentials_contract(sources_engine, built):
    """docs/13: the app hands the Notion connection over; a bad body is 400, disconnecting answers the state. Connecting
    is left to the engine tests (it starts a refresh that would call Notion)."""
    e = sources_engine
    for bad in ({}, {"token": 3}, {"token": "a b"}, {"token": "t", "workspace": 1}):
        status, _ = call(e.url + "/notion/credentials", "POST", bad)
        assert status == 400, bad
    assert call(e.url + "/notion/credentials", "POST", {"token": None}) == (
        200, {"connected": False, "workspace": None})
    status, ix = call(e.url + "/index")
    assert status == 200
    assert ix["notion"] == {"connected": False, "workspace": None, "refused": False, "pages": None,
                            "fetched_at": None}


def test_meetings_contract(sources_engine, built):
    """docs/30: the app sends its finished meetings; a bad body is 400. A good list is left
    to the engine tests (it starts a refresh of this shared engine)."""
    e = sources_engine
    for bad in ({}, {"meetings": "x"}, {"meetings": [{"id": "a"}]},
                {"meetings": [{"id": 1, "title": "t", "start": "s", "end": "e"}]}):
        status, _ = call(e.url + "/meetings", "POST", bad)
        assert status == 400, bad


def test_note_folders_contract(sources_engine, built):
    """docs/34: the app sends the note folders the user chose; a bad body is 400. A good list is left to the engine
    tests (it starts a refresh of this shared engine). `/index` reports the notes with these keys."""
    e = sources_engine
    for bad in ({}, {"folders": "x"}, {"folders": [{"path": "relative"}]},
                {"folders": [{"path": "/a", "vault_id": "short"}]}):
        status, _ = call(e.url + "/notes/folders", "POST", bad)
        assert status == 400, bad
    status, ix = call(e.url + "/index")
    assert status == 200
    assert sorted(ix["notes"]) == ["chosen", "folders", "not_downloaded", "notes", "read_at"]


def test_linear_credentials_contract(sources_engine, built):
    """docs/18: the app hands the Linear connection over; a bad body is 400 and never echoes the input, disconnecting
    answers the state. Connecting is left to the engine tests (it starts a refresh that would call Linear)."""
    e = sources_engine
    for bad in ({}, {"token": 3}, {"token": "a b"}, {"token": "t", "workspace": 1}, {"token": "t", "muted_teams": "t1"},
                {"token": "café"}):
        status, out = call(e.url + "/linear/credentials", "POST", bad)
        assert status == 400, bad
        assert "café" not in json.dumps(out, ensure_ascii=False)
    assert call(e.url + "/linear/credentials", "POST", {"token": None}) == (
        200, {"connected": False, "workspace": None})
    status, ix = call(e.url + "/index")
    assert status == 200
    exactly((200, ix["linear"]), 200, {"connected": False, "workspace": None, "refused": False, "issues": None,
                                       "teams": None, "fetched_at": None, "error_kind": None, "lost_since": None})


def test_linear_teams_contract(sources_engine, built):
    """docs/18 10절: the team list is empty before anything was fetched."""
    assert call(sources_engine.url + "/linear/teams") == (200, {"teams": []})


def test_gdrive_credentials_contract(sources_engine, built):
    """docs/19: the app hands the Google Drive connection over; a bad body is 400 and never echoes the input,
    disconnecting answers the state. Connecting is left to the engine tests (it starts a refresh that would call Drive)."""
    e = sources_engine
    for bad in ({}, {"token": 3}, {"token": "a b"}, {"token": "t", "account": 1}, {"token": "t", "muted_drives": "d1"},
                {"token": "café"}):
        status, out = call(e.url + "/gdrive/credentials", "POST", bad)
        assert status == 400, bad
        assert "café" not in json.dumps(out, ensure_ascii=False)
    assert call(e.url + "/gdrive/credentials", "POST", {"token": None}) == (
        200, {"connected": False, "account": None})
    status, ix = call(e.url + "/index")
    assert status == 200
    exactly((200, ix["gdrive"]), 200, {"connected": False, "account": None, "refused": False, "files": None,
                                       "drives": None, "fetched_at": None, "initial_done": None, "error_kind": None,
                                       "lost_since": None})


def test_gdrive_drives_contract(sources_engine, built):
    """docs/19 11절: the shared drive list is empty before anything was fetched."""
    assert call(sources_engine.url + "/gdrive/drives") == (200, {"drives": []})


def test_dropbox_credentials_contract(sources_engine, built):
    """docs/33 6절: the app hands the Dropbox connection over; a bad body is 400 and never echoes the input, a token
    always comes with its account_id, disconnecting answers the state. Connecting is left to the engine tests (it starts
    a refresh that would call Dropbox)."""
    e = sources_engine
    for bad in ({}, {"token": 3}, {"token": "a b"}, {"token": "t", "account": 1}, {"token": "t", "muted_folders": "f1"},
                {"token": "café"}, {"token": "t"}, {"token": "t", "account": "me@example.com"}):
        status, out = call(e.url + "/dropbox/credentials", "POST", bad)
        assert status == 400, bad
        assert "café" not in json.dumps(out, ensure_ascii=False)
    assert call(e.url + "/dropbox/credentials", "POST", {"token": None}) == (200, {"connected": False, "account": None})
    status, ix = call(e.url + "/index")
    assert status == 200
    exactly((200, ix["dropbox"]), 200, {"connected": False, "account": None, "files": None, "fetched_at": None,
                                        "initial_done": None, "refused": False, "error_kind": None, "lost_since": None})


def test_dropbox_folders_contract(sources_engine, built):
    """docs/33 4절 3: the top folder list is empty before anything was fetched."""
    assert call(sources_engine.url + "/dropbox/folders") == (200, {"folders": []})


def test_atlassian_credentials_contract(sources_engine, built):
    """docs/31 6절: the app hands the Jira and Confluence connection over; a bad body is 400 and never echoes the
    input, disconnecting answers the state. Connecting is left to the engine tests (it starts a refresh that would call
    Atlassian)."""
    e = sources_engine
    for bad in ({}, {"token": 3}, {"token": "a b"}, {"token": "t", "cloud": 1}, {"token": "t", "site": ["café"]},
                {"token": "t", "site_url": False}, {"token": "t", "muted_projects": "p1"},
                {"token": "t", "muted_spaces": [1]}, {"token": "café"}):
        status, out = call(e.url + "/atlassian/credentials", "POST", bad)
        assert status == 400, bad
        assert "café" not in json.dumps(out, ensure_ascii=False)
    assert call(e.url + "/atlassian/credentials", "POST", {"token": None}) == (
        200, {"connected": False, "site": None})
    status, ix = call(e.url + "/index")
    assert status == 200
    exactly((200, ix["atlassian"]), 200, {"connected": False, "site": None, "refused": False, "issues": None,
                                          "pages": None, "fetched_at": None, "initial_done": None,
                                          "error_kind": None, "lost_since": None, "paused_until": None})


def test_atlassian_scopes_contract(sources_engine, built):
    """docs/31 8절: the project and space lists are empty before anything was fetched."""
    exactly(call(sources_engine.url + "/atlassian/scopes"), 200, {"projects": [], "spaces": []})


def test_asana_credentials_contract(sources_engine, built):
    """docs/32 6절: the app hands the Asana connection over; a bad body is 400 and never echoes the input,
    disconnecting answers the state. Connecting is left to the engine tests (it starts a refresh that would call
    Asana)."""
    e = sources_engine
    for bad in ({}, {"token": 3}, {"token": "a b"}, {"token": "t", "user_gid": 1}, {"token": "t", "user_gid": "café"},
                {"token": "t", "muted_workspaces": "w1"}, {"token": "t", "muted_projects": [1]}, {"token": "café"}):
        status, out = call(e.url + "/asana/credentials", "POST", bad)
        assert status == 400, bad
        assert "café" not in json.dumps(out, ensure_ascii=False)
    assert call(e.url + "/asana/credentials", "POST", {"token": None}) == (200, {"connected": False})
    status, ix = call(e.url + "/index")
    assert status == 200
    exactly((200, ix["asana"]), 200, {"connected": False, "refused": False, "workspaces": None, "tasks": None,
                                      "fetched_at": None, "initial_done": None, "error_kind": None,
                                      "lost_since": None, "blocked_workspaces": None})


def test_asana_scopes_contract(sources_engine, built):
    """docs/32 6절: the workspace and project lists are empty before anything was fetched."""
    exactly(call(sources_engine.url + "/asana/scopes"), 200, {"workspaces": [], "projects": []})
