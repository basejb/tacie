"""A fake GitHub REST server for the contract tests of /index* and /github/* (spec 7-2). The engine reaches it through
TACIE_GITHUB_API_URL (a loopback address, which a release build accepts), so no request leaves the machine and the
engine's own HTTP client reads every answer.

Two ThreadingHTTPServers on 127.0.0.1:0: the API, and codeload (the tarball's 303 points there, as GitHub's does).

Route table: a JSON file (TACIE_FAKE_GITHUB, or `routes=`) read again on every request, so a test swaps it with
set_routes. Keys are API paths without the leading `/`, query allowed (`repos/o/n/issues?state=all`). A request
matches a key when the path is the same and every query value of the key is in the request; `per_page`, `page` and
`after` are left out of matching on both sides, values are compared percent-decoded. When several keys match, the one
with more query values wins. A value is
  {"status"?: 200, "headers"?: {...}, "body"?: JSON}   the JSON body; a list is cut into pages by `per_page`, with
                                                       `Link: <…repositories/{id}/…&page=n>; rel="next"` as GitHub
                                                       gives it; 304 with no body when If-None-Match is the ETag header
  {"status"?: 200, "raw": "text"}                      the text as it is (a cut-off answer)
  {"close": true}                                      the connection is closed with no answer (network)
`POST graphql` answers the "graphql" entry ({"data": {"repository": {}}} without one).

Repository content: when no route matches, the work folder (TACIE_FAKE_GITHUB_WORK, or `work=`) answers for the repo
`repo` (default o/n), read at the request: `repos/o/n`, `git/ref/heads/main` (the work folder's commit; other branches
404), `git/trees/{commit or tree}[?recursive=1]` (paths in reverse order, as the Rust fake gives them),
`git/blobs/{sha}` (raw bytes for a `raw` Accept, else base64 JSON) and `tarball/{commit}` (303 to codeload
`…/legacy.tar.gz/{sha}?token=t`, a gzip tar whose top folder is `o-n-<sha>/`). Files are taken in path byte order,
an executable is 100755, a link 120000. Blob SHAs are git's; tree and commit SHAs are sha1 of the listing (not real
git objects). Every commit served is kept, so an older commit still answers after the folder changed.

Only the tokens in TOKENS are let in (401 Bad credentials otherwise). Every request appends one JSON line
{"method", "path", "query", "has_token", "if_none_match", "authorization_on_codeload"} to TACIE_FAKE_GITHUB_LOG (or
`log=`): `authorization_on_codeload` is null for API requests and says whether codeload got an Authorization header for
codeload requests (whose query, the temporary token, is not written). No token value is ever written. A request no
route and no content answered, or a `repositories/{id}` link with an id this fake never gave, gets 500 and is kept in
`unanswered`. Both servers keep a listen backlog of 128.
"""
import base64
import gzip
import hashlib
import io
import json
import os
import stat
import sys
import tarfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote

TOKEN = "gho_contractSecret0123456789"  # the Tacie token the tests connect with; it must never show in a log
GH_TOKEN = "test-gh-token"  # what the fake `gh auth token` prints (gh mode)
TOKENS = (TOKEN, GH_TOKEN)
ISSUE_MARK = "LEAKMARK-7f3"  # ASCII, so a log that escapes or re-encodes Korean still shows it
# the one issue of o/n, as GitHub's REST list gives it (only what the engine reads)
ISSUE = {"number": 1, "title": "연결 확인", "state": "open", "user": {"login": "sora-p", "type": "User"},
         "created_at": "2026-10-01T00:00:00Z", "updated_at": "2026-10-01T00:00:00Z", "body": f"설명이에요. {ISSUE_MARK}",
         "comments": 0, "labels": [], "html_url": "https://github.com/o/n/issues/1",
         "repository_url": "https://api.github.com/repos/o/n"}
# probe.rs PROBES: each stream's newest item, asked by /index/check with the saved ETag
PROBES = {"issues": "repos/o/n/issues?state=all&sort=updated&direction=desc&per_page=1",
          "comments": "repos/o/n/issues/comments?sort=updated&direction=desc&per_page=1",
          "review_comments": "repos/o/n/pulls/comments?sort=updated&direction=desc&per_page=1"}
LOGIN = "basejb"
BASE = {
    "repos/o/n": {"body": {"id": 1, "full_name": "o/n", "default_branch": "main", "private": True}},
    "repos/o/n/issues?state=all": {"body": [ISSUE]},
    "repos/o/n/issues/comments": {"body": []},
    "repos/o/n/pulls/comments": {"body": []},
    PROBES["issues"]: {"body": [ISSUE], "headers": {"ETag": '"e-issues"'}},
    PROBES["comments"]: {"body": [], "headers": {"ETag": '"e-comments"'}},
    PROBES["review_comments"]: {"body": [], "headers": {"ETag": '"e-review-comments"'}},
    "user": {"body": {"login": LOGIN}},
    "graphql": {"body": {"data": {"repository": {}}}},
}
PAGING = ("per_page", "page", "after")


def _pairs(query):
    out = []
    for p in query.split("&"):
        if p:
            k, _, v = p.partition("=")
            out.append((unquote(k.replace("+", " ")), unquote(v.replace("+", " "))))
    return out


def _sha1(data):
    return hashlib.sha1(data).hexdigest()


def blob_sha(data):
    return _sha1(b"blob %d\0" % len(data) + data)


class _Commit:
    """One commit of the work folder: files (path, mode, bytes) in path byte order, trees {sha: [items]}, blobs."""

    def __init__(self, files):
        self.files, self.trees, self.blobs = files, {}, {}
        self.root = self._tree("", files)
        listing = "\n".join(f"{m} {p} {blob_sha(b)}" for p, m, b in files)
        self.sha = _sha1(f"commit {listing}".encode())

    def _tree(self, prefix, files):
        items, folders = [], []
        for path, mode, data in files:
            if not path.startswith(prefix):
                continue
            rest = path[len(prefix):]
            if "/" in rest:
                d = rest.split("/", 1)[0]
                if d not in folders:
                    folders.append(d)
            else:
                sha = blob_sha(data)
                self.blobs[sha] = data
                items.append({"name": rest, "mode": mode, "type": "blob", "sha": sha})
        for d in folders:
            items.append({"name": d, "mode": "040000", "type": "tree", "sha": self._tree(f"{prefix}{d}/", files)})
        sha = _sha1(("tree " + "\n".join(f"{i['mode']} {i['name']} {i['sha']}" for i in items)).encode())
        self.trees[sha] = items
        return sha

    def listing(self, sha, recursive):
        out, todo = [], [("", self.trees[sha])]
        while todo:
            prefix, items = todo.pop()
            for i in items:
                out.append({"path": prefix + i["name"], "mode": i["mode"], "type": i["type"], "sha": i["sha"]})
                if recursive and i["type"] == "tree":
                    todo.append((prefix + i["name"] + "/", self.trees[i["sha"]]))
        return sorted(out, key=lambda e: e["path"], reverse=True)

    def tarball(self, top):
        buf = io.BytesIO()
        with gzip.GzipFile(fileobj=buf, mode="wb", mtime=0) as gz, \
                tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT,
                             pax_headers={"comment": self.sha}) as tar:
            made = set()

            def folder(name):
                ti = tarfile.TarInfo(name)
                ti.type, ti.mode = tarfile.DIRTYPE, 0o775
                tar.addfile(ti)

            folder(top)
            for path, mode, data in self.files:
                parts = path.split("/")
                for n in range(1, len(parts)):
                    d = "/".join(parts[:n])
                    if d not in made:
                        made.add(d)
                        folder(f"{top}/{d}")
                ti = tarfile.TarInfo(f"{top}/{path}")
                if mode == "120000":
                    ti.type, ti.linkname, ti.mode = tarfile.SYMTYPE, data.decode("utf-8", "surrogateescape"), 0o777
                    tar.addfile(ti)
                else:
                    ti.size, ti.mode = len(data), 0o775 if mode == "100755" else 0o664
                    tar.addfile(ti, io.BytesIO(data))
        return buf.getvalue()


def read_work(work):
    """The work folder's files: (path, mode, bytes) in path byte order (git's ls-files order)."""
    work = Path(work)
    files = []
    for p in work.rglob("*"):
        if p.is_symlink():
            files.append((p.relative_to(work).as_posix(), "120000", os.fsencode(os.readlink(p))))
        elif p.is_file():
            mode = "100755" if p.stat().st_mode & stat.S_IXUSR else "100644"
            files.append((p.relative_to(work).as_posix(), mode, p.read_bytes()))
    files.sort(key=lambda f: f[0].encode())
    return files


class _Server(ThreadingHTTPServer):
    """A deep listen backlog: a burst of blob requests (BLOB_WORKERS at once, plus the tarball) never gets refused."""
    request_queue_size = 128
    daemon_threads = True


class FakeGitHub:
    def __init__(self, routes=None, work=None, log=None, repo="o/n"):
        self.routes_file = routes or os.environ.get("TACIE_FAKE_GITHUB")
        self.work = work or os.environ.get("TACIE_FAKE_GITHUB_WORK")
        self.log_file = log or os.environ.get("TACIE_FAKE_GITHUB_LOG")
        self.repo = repo
        self.commits = {}  # sha → _Commit, every commit served so far
        self.repo_ids = []  # `repos/o/n` → the id in the next-page links (1, 2, …)
        self.unanswered = []
        self._lock = threading.Lock()
        self.api = _Server(("127.0.0.1", 0), _handler(self, codeload=False))
        self.codeload = _Server(("127.0.0.1", 0), _handler(self, codeload=True))
        for s in (self.api, self.codeload):
            threading.Thread(target=s.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.api.server_address[1]}/"
        self.codeload_url = f"http://127.0.0.1:{self.codeload.server_address[1]}/"

    def close(self):
        for s in (self.api, self.codeload):
            s.shutdown()
            s.server_close()

    # --- content ---

    def head(self):
        """The work folder's commit now (and keeps it, so it answers later too)."""
        c = _Commit(read_work(self.work))
        with self._lock:
            self.commits.setdefault(c.sha, c)
        return c.sha

    def _commit_of(self, sha):
        with self._lock:
            c = self.commits.get(sha)
            if c:
                return c, c.root
            for c in self.commits.values():
                if sha in c.trees:
                    return c, sha
        return None, None

    # --- requests ---

    def routes(self):
        if not self.routes_file:
            return dict(BASE)
        return json.loads(Path(self.routes_file).read_text(encoding="utf-8"))

    def log(self, rec):
        if not self.log_file:
            return
        line = json.dumps(rec, ensure_ascii=False) + "\n"
        with self._lock, open(self.log_file, "a", encoding="utf-8") as f:
            f.write(line)

    def route(self, path, query):
        table = self.routes()
        if path == "graphql":
            return table.get("graphql", BASE["graphql"])
        want = [kv for kv in _pairs(query) if kv[0] not in PAGING]
        best, best_n = None, -1
        for key, value in table.items():
            kpath, _, kquery = key.partition("?")
            if kpath != path:
                continue
            need = [kv for kv in _pairs(kquery) if kv[0] not in PAGING]
            if all(kv in want for kv in need) and len(need) > best_n:
                best, best_n = value, len(need)
        return best

    def next_link(self, path, query, page):
        parts = path.split("/", 3)
        prefix, tail = ("/".join(parts[:3]), parts[3]) if len(parts) == 4 and parts[0] == "repos" else ("", path)
        with self._lock:
            if prefix not in self.repo_ids:
                self.repo_ids.append(prefix)
            rid = self.repo_ids.index(prefix) + 1
        kept = "&".join(p for p in query.split("&") if p and p.partition("=")[0] not in ("page", "after"))
        return f'<{self.url}repositories/{rid}/{tail}?{kept}&after=c{page}&page={page}>; rel="next"'

    def original_path(self, path):
        """The path a `repositories/{id}/…` next-page link stands for, or None for an id this fake never gave out."""
        if not path.startswith("repositories/"):
            return path
        rid, _, tail = path[len("repositories/"):].partition("/")
        with self._lock:
            if not rid.isdigit() or not 1 <= int(rid) <= len(self.repo_ids):
                return None
            prefix = self.repo_ids[int(rid) - 1]
        return f"{prefix}/{tail}" if prefix else tail

    def content(self, path, query, accept):
        """(status, headers, body bytes) from the work folder, or None."""
        base = f"repos/{self.repo}"
        if path.lower() != base.lower() and not path.lower().startswith(base.lower() + "/"):
            return None
        rest = path[len(base):].lstrip("/")
        js = lambda v, status=200: (status, {"Content-Type": "application/json"}, json.dumps(v).encode())
        not_found = js({"message": "Not Found"}, 404)
        if rest == "":
            return js({"id": 1, "name": self.repo.split("/")[1], "full_name": self.repo, "default_branch": "main",
                       "private": True})
        if rest.startswith("git/ref/heads/"):
            if unquote(rest[len("git/ref/heads/"):]) != "main":
                return not_found
            return js({"ref": "refs/heads/main", "object": {"sha": self.head(), "type": "commit"}})
        if rest.startswith("git/trees/"):
            sha = rest[len("git/trees/"):]
            c, tree = self._commit_of(sha)
            if c is None:
                return js({"message": "Invalid object requested. SHA must identify a commit or a tree."}, 422)
            recursive = any(k == "recursive" for k, _ in _pairs(query))
            return js({"sha": tree, "tree": c.listing(tree, recursive), "truncated": False})
        if rest.startswith("git/blobs/"):
            sha = rest[len("git/blobs/"):]
            with self._lock:
                data = next((c.blobs[sha] for c in self.commits.values() if sha in c.blobs), None)
            if data is None:
                return not_found
            if "raw" in (accept or ""):
                return 200, {"Content-Type": "text/plain; charset=utf-8"}, data
            return js({"sha": sha, "size": len(data), "encoding": "base64",
                       "content": base64.b64encode(data).decode()})
        if rest.startswith("tarball/"):
            sha = rest[len("tarball/"):]
            with self._lock:
                known = sha in self.commits
            if not known:
                return not_found
            return 303, {"Location": f"{self.codeload_url}{self.repo}/legacy.tar.gz/{sha}?token=t"}, b""
        return None


def _handler(fake, codeload):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):  # nothing on pytest's stderr
            pass

        def _send(self, status, headers, body):
            self.send_response(status)
            for k, v in headers.items():
                self.send_header(k, v)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            self.close_connection = True

        def do_GET(self):
            self._serve("GET")

        def do_POST(self):
            self._serve("POST")

        def _serve(self, method):
            path, _, query = self.path.lstrip("/").partition("?")
            if method == "POST":
                self.rfile.read(int(self.headers.get("Content-Length") or 0))
            auth = self.headers.get("Authorization")
            if codeload:
                fake.log({"method": method, "path": path, "query": "", "has_token": False,
                          "if_none_match": None, "authorization_on_codeload": auth is not None})
                name, _, sha = path.partition("/legacy.tar.gz/")
                c = fake.commits.get(sha) if name.lower() == fake.repo.lower() else None
                if c is None:
                    return self._send(404, {}, b"")
                return self._send(200, {"Content-Type": "application/x-gzip"},
                                  c.tarball(f"{fake.repo.replace('/', '-')}-{sha}"))
            inm = self.headers.get("If-None-Match")
            has_token = auth in [f"Bearer {t}" for t in TOKENS]
            fake.log({"method": method, "path": path, "query": query, "has_token": has_token,
                      "if_none_match": inm, "authorization_on_codeload": None})
            if not has_token:
                return self._send(401, {"Content-Type": "application/json"},
                                  json.dumps({"message": "Bad credentials"}).encode())
            known = fake.original_path(path)
            if known is None:  # a next-page link this fake never gave: unanswered, like any other unknown request
                with fake._lock:
                    fake.unanswered.append(f"{method} {path}" + (f"?{query}" if query else ""))
                return self._send(500, {}, b"")
            path = known
            route = fake.route(path, query) if method in ("GET", "POST") else None
            if route is None and method == "GET":
                got = fake.content(path, query, self.headers.get("Accept"))
                if got is not None:
                    return self._send(*got)
            if route is None:
                with fake._lock:
                    fake.unanswered.append(f"{method} {path}" + (f"?{query}" if query else ""))
                return self._send(500, {}, b"")
            if route.get("close"):
                self.close_connection = True
                return
            status = route.get("status", 200)
            headers = dict(route.get("headers") or {})
            etag = next((v for k, v in headers.items() if k.lower() == "etag"), None)
            if etag and inm == etag:
                return self._send(304, headers, b"")
            if "raw" in route:
                return self._send(status, {"Content-Type": "application/json", **headers}, route["raw"].encode())
            body = route.get("body")
            pairs = dict(_pairs(query))
            if isinstance(body, list) and pairs.get("per_page", "").isdigit():
                per, page = max(1, int(pairs["per_page"])), int(pairs.get("page") or 1)
                start = (page - 1) * per
                if start + per < len(body):
                    headers["Link"] = fake.next_link(path, query, page + 1)
                body = body[start:start + per]
            text = b"" if body is None else json.dumps(body, ensure_ascii=False).encode()
            return self._send(status, {"Content-Type": "application/json; charset=utf-8", **headers}, text)

    return Handler


if __name__ == "__main__":  # run alone: prints {"url": …} and serves until stdin closes
    fake = FakeGitHub()
    print(json.dumps({"url": fake.url}), flush=True)
    sys.stdin.read()
    fake.close()
