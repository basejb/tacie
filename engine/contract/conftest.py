"""Contract tests: the engine's HTTP contract, as the app sees it, checked against the Rust engine binary.

TACIE_ENGINE_BIN names the binary (without it every contract test skips):
    cd engine && cargo build --release && cd .. &&
    TACIE_ENGINE_BIN=engine/target/release/tacie-engine uv run pytest -q engine/contract
The model comes from TACIE_MODEL_DIR, else its default folder (~/Library/Application Support/Tacie/models/bge-m3-onnx).
The engine runs the way the app runs it (a subprocess that prints {"port": N, ...} on its first stdout line and quits
when stdin closes), on the tacie-index crate's 6-chunk fixture index (the /index* and /github/* tests: an engine of its
own on a temp root against fake_github.py, see sources_engine), with a fresh HOME whose key files hold fake keys and
with Jev and OpenRouter pointed at the fakes in fakes.py. Nothing is written to data/ or index/ (real_data_untouched),
and no request leaves 127.0.0.1. The Python engine these tests were first written against was deleted on 2026-10-26;
reference.py keeps its card() and model defaults.
"""
import collections
import contextlib
import json
import os
import signal
import subprocess
import threading
import time
from pathlib import Path
from types import SimpleNamespace

import pytest

from . import fake_github
from .fakes import fake_jev, fake_openrouter

REPO = Path(__file__).resolve().parents[2]
MINI = REPO / "engine/crates/tacie-index/tests/fixtures/mini"
TOKEN = "contract"
FAKE_KEY = "contract-fake"  # in the fresh HOME's key files; the fakes see it as "Bearer contract-fake"
START_S = 120  # the engine loads bge-m3 in 1–3 s; a cold disk cache can take much longer
STOP_S = 5


BINARY = os.environ.get("TACIE_ENGINE_BIN")
BINARY = BINARY and str(Path(BINARY).resolve())


def pytest_collection_modifyitems(config, items):
    """Without TACIE_ENGINE_BIN every test in this folder skips (other folders in the same run are left alone)."""
    if BINARY:
        return
    skip = pytest.mark.skip(
        reason="set TACIE_ENGINE_BIN (cd engine && cargo build --release → engine/target/release/tacie-engine)")
    here = Path(__file__).resolve().parent
    for item in items:
        if here in Path(item.path).resolve().parents:
            item.add_marker(skip)


def _listing(d):
    return {p.name: p.stat().st_mtime_ns for p in d.iterdir()} if d.is_dir() else None


def _snapshot():
    cache = REPO / "data/cache"
    files = {str(p): p.stat().st_mtime_ns for p in cache.rglob("*") if p.is_file()} if cache.is_dir() else None
    return [_listing(REPO / d) for d in ("data/sources", "data/sources/github", "data/sources/github/items", "index")], files


@pytest.fixture(autouse=True)
def real_data_untouched():
    """No test may touch the real data folders (top level of each, and every cache file)."""
    before = _snapshot()
    yield
    assert _snapshot() == before, "a test changed the real data/ or index/"


@pytest.fixture(scope="session")
def fakes():
    jev, openrouter = fake_jev(), fake_openrouter()
    yield SimpleNamespace(jev=jev, openrouter=openrouter)
    jev.close()
    openrouter.close()


@pytest.fixture(autouse=True)
def _fresh_fakes(fakes):
    """Each test starts with both fakes answering (mode high), Jev's gate plans empty and no recorded requests."""
    fakes.jev.reset()
    fakes.openrouter.reset()
    yield
    fakes.jev.reset()
    fakes.openrouter.reset()


def _env(home, fakes):
    """The shell's environment minus every TACIE_* override, with a fresh HOME and the fakes' addresses."""
    real_home = Path.home()  # the model stays where it is, outside the fresh HOME
    env = {k: v for k, v in os.environ.items() if not k.startswith("TACIE_")}
    # TACIE_KEY_FILES=1: the dev app's engine, which also reads the key files (test_keys.py starts a product one)
    env.update(HOME=str(home), TACIE_ENGINE_TOKEN=TOKEN, TACIE_JEV_URL=fakes.jev.url, TACIE_KEY_FILES="1",
               TACIE_OPENROUTER_URL=fakes.openrouter.url,
               TACIE_MODEL_DIR=(os.environ.get("TACIE_MODEL_DIR")
                                or str(real_home / "Library/Application Support/Tacie/models/bge-m3-onnx")))
    return env


def _fresh_home(root):
    """Fake TypeSafe and OpenRouter keys (so /health says typesafe and /config shows both ok), no Slack key."""
    for name in ("typesafe", "openrouter"):
        d = root / ".config" / name
        d.mkdir(parents=True)
        (d / "config.json").write_text(json.dumps({"apiKey": FAKE_KEY}))
    return root


def _drain(stream, keep):
    for line in iter(stream.readline, ""):
        keep.append(line.rstrip("\n"))
    stream.close()


def _stop(proc):
    """Close stdin (the engine quits on EOF), wait, then kill its whole process group."""
    try:
        proc.stdin.close()
    except OSError:  # the engine is gone already
        pass
    try:
        proc.wait(timeout=STOP_S)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait(timeout=STOP_S)


class _Running:
    """An engine started the way the app starts it: its first stdout line parsed, stderr kept in `stderr`."""

    def __init__(self, cmd, cwd, env, keep=200):
        self.cmd = cmd
        self.proc = subprocess.Popen(cmd, cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True, encoding="utf-8", start_new_session=True)
        self.stdout, self.stderr = [], collections.deque(maxlen=keep)
        self._readers = [threading.Thread(target=_drain, args=(stream, kept), daemon=True)  # a full pipe would stall it
                         for stream, kept in ((self.proc.stdout, self.stdout), (self.proc.stderr, self.stderr))]
        for r in self._readers:
            r.start()

    def first_line(self):
        """The port line, once the index is loaded; the test fails with the engine's last stderr lines otherwise."""
        proc, stdout = self.proc, self.stdout
        deadline = time.monotonic() + START_S
        while not stdout and proc.poll() is None and time.monotonic() < deadline:
            time.sleep(0.05)
        if not stdout:
            code = proc.poll()
            self.stop()
            tail = "\n".join(list(self.stderr)[-20:])
            why = f"exited with {code}" if code is not None else f"printed no port line in {START_S} s"
            pytest.fail(f"engine {why}: {' '.join(self.cmd)}\n--- last stderr lines ---\n{tail}")
        try:
            first = json.loads(stdout[0])
            first["port"]
        except (json.JSONDecodeError, TypeError, KeyError):
            self.stop()
            pytest.fail(f"engine's first stdout line is not {{\"port\": N, ...}}: {stdout[0]!r}")
        return first

    def stop(self):
        _stop(self.proc)
        self._readers[1].join(timeout=2)  # the last lines may still be in the pipe


@pytest.fixture(scope="session")
def engine(fakes, tmp_path_factory):
    cmd = [BINARY, "--index-dir", str(MINI), "--port", "0", "--exit-on-stdin-eof"]
    home = _fresh_home(tmp_path_factory.mktemp("home"))
    run = _Running(cmd, REPO, _env(home, fakes))
    first = run.first_line()
    try:
        yield SimpleNamespace(url=f"http://127.0.0.1:{first['port']}", key=FAKE_KEY, first_line=first,
                              jev=fakes.jev, openrouter=fakes.openrouter,
                              chunks=json.loads((MINI / "chunks.json").read_text()),
                              meta=json.loads((MINI / "meta.json").read_text()))
    finally:
        run.stop()


# --- /index* and /github/*: an engine of its own on a temp root, a fake GitHub server, a recording gh and git ---

SOURCES_META = {"repo": "o/n", "sources": {"repo": {"path": "data/sources/o__n", "branch": "main"},
                                           "github": {"repo": "o/n"}}}
README_MARK = "LEAKMARK-a91"  # ASCII content markers: the logs must not carry them (test_logs_carry_no_content)
README = f"# o/n\n\no/n 저장소의 설명이에요. {README_MARK}\n"
APP = "NAME = 'o/n'\n"


def _sources_root(root):
    """work/ (README.md and app.py: the fake GitHub's o/n main), an empty index/ whose meta names o/n, a folder of
    another repo's that must stay (data/sources/repos/x__y/keep), and bin/gh, bin/git."""
    import numpy as np  # only this fixture needs it (the empty emb.npy)

    work = root / "work"
    work.mkdir()
    (work / "README.md").write_text(README, encoding="utf-8")
    (work / "app.py").write_text(APP, encoding="utf-8")
    keep = root / "data/sources/repos/x__y/keep"
    keep.parent.mkdir(parents=True)
    keep.write_text("keep\n")
    index = root / "index"
    index.mkdir()
    (index / "chunks.json").write_text("[]")
    np.save(index / "emb.npy", np.zeros((0, 1024), np.float32))
    (index / "meta.json").write_text(json.dumps(SOURCES_META, indent=1, ensure_ascii=False), encoding="utf-8")
    bin_dir = root / "bin"
    bin_dir.mkdir()
    # sh builtins only (printf, test, exit): they run on a PATH without /usr/bin too
    (bin_dir / "gh").write_text(f'''#!/bin/sh
printf '%s\\n' "$*" >> "{root / 'gh.log'}"
if [ "$*" = "auth token" ]; then printf '%s\\n' "{fake_github.GH_TOKEN}"; exit 0; fi
exit 1
''')
    (bin_dir / "git").write_text(f'''#!/bin/sh
printf '%s\\n' "$*" >> "{root / 'git.log'}"
exit 1
''')
    for f in ("gh", "git"):
        (bin_dir / f).chmod(0o755)
    return work


@pytest.fixture(scope="module")
def sources_engine(fakes, tmp_path_factory):
    """The engine on a temp root (see _sources_root) talking to a fake GitHub (fake_github.py), with a recording gh
    and git first on PATH. See start_sources_engine for what it gives."""
    with start_sources_engine(fakes, tmp_path_factory, mode="gh") as e:
        yield e


@contextlib.contextmanager
def start_sources_engine(fakes, tmp_path_factory, prepare=None, mode=None):
    """sources_engine's body. `prepare(root)` changes the temp root before the engine starts; `mode` is
    TACIE_GITHUB_MODE: "gh" (the dev app's launch: the recording gh gives the token), "none" (the product launch),
    or None (unset: the engine's default, none; gh mode is then refused). Gives `url`, `root`, `work`, `github` (the fake),
    `head()` (the work folder's commit as the fake serves it), `set_routes(dict)` (fake_github.BASE with these on top;
    {} puts BASE back), `push(files)` (writes {path: text} into work/: the next commit), `api_calls()` (the fake's
    API requests, oldest first), `codeload_calls()` (its codeload requests), `gh_calls()` and `git_calls()` (argv lists) and `stderr()` (every engine line)."""
    root = tmp_path_factory.mktemp("sources")
    work = _sources_root(root)
    if prepare:
        prepare(root)
    cmd = [BINARY, "--index-dir", "index", "--data-dir", "data", "--port", "0", "--exit-on-stdin-eof"]
    routes, api_log = root / "github-routes.json", root / "github.log"
    gh_log, git_log = root / "gh.log", root / "git.log"

    def set_routes(extra):
        tmp = routes.with_name(routes.name + ".tmp")
        tmp.write_text(json.dumps({**fake_github.BASE, **extra}, ensure_ascii=False), encoding="utf-8")
        os.replace(tmp, routes)  # the fake reads the table on every request: never half of it

    def push(files):
        for path, text in files.items():
            f = work / path
            f.parent.mkdir(parents=True, exist_ok=True)
            f.write_text(text, encoding="utf-8")

    def lines(f):
        return f.read_text(encoding="utf-8").splitlines() if f.exists() else []

    set_routes({})
    github = fake_github.FakeGitHub(routes=str(routes), work=str(work), log=str(api_log))
    env = _env(_fresh_home(tmp_path_factory.mktemp("home")), fakes)
    for k in ("GH_TOKEN", "GITHUB_TOKEN"):
        env.pop(k, None)
    env.update(PATH=f"{root / 'bin'}{os.pathsep}{env.get('PATH', '')}", TACIE_GITHUB_API_URL=github.url)
    if mode:
        env["TACIE_GITHUB_MODE"] = mode
    run = _Running(cmd, root, env, keep=None)  # every line: the tests look for content in all of them
    try:
        first = run.first_line()
        yield SimpleNamespace(url=f"http://127.0.0.1:{first['port']}", root=root, work=work, github=github,
                              head=github.head, set_routes=set_routes, push=push,
                              api_calls=lambda: [c for c in map(json.loads, lines(api_log))
                                                 if c["authorization_on_codeload"] is None],
                              codeload_calls=lambda: [c for c in map(json.loads, lines(api_log))
                                                      if c["authorization_on_codeload"] is not None],
                              gh_calls=lambda: [ln.split(" ") for ln in lines(gh_log)],
                              git_calls=lambda: [ln.split(" ") for ln in lines(git_log)],
                              stderr=lambda: list(run.stderr))
    finally:
        run.stop()
        github.close()
    assert github.unanswered == [], f"the fake GitHub got requests nobody answered: {github.unanswered}"
    assert lines(git_log) == [], f"the engine ran git: {lines(git_log)}"  # spec 7-2: nothing may run git
