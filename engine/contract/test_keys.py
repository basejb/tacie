"""AI keys handed over by the app (#7 step 5): POST /ai/credentials holds them in memory only.

A product engine (no TACIE_KEY_FILES) never reads ~/.config key files, even when the fresh HOME has them: until the app
hands over keys every AI call fails with error_kind `key` and nothing reaches the fakes. The keys never show up in an
answer or on stderr.
"""
import pytest

from .conftest import BINARY, MINI, REPO, _env, _fresh_home, _Running
from .test_core import ASK_DOWN_KEYS, QUESTION, call

APP_TS, APP_OR = "app-ts-secret-1", "app-or-secret-2"


@pytest.fixture(scope="module")
def product(fakes, tmp_path_factory):
    home = _fresh_home(tmp_path_factory.mktemp("home"))  # key files with the conftest's fake key: must stay unread
    env = _env(home, fakes)
    env.pop("TACIE_KEY_FILES")
    run = _Running([BINARY, "--index-dir", str(MINI), "--port", "0", "--exit-on-stdin-eof"], REPO, env)
    first = run.first_line()
    try:
        yield run, f"http://127.0.0.1:{first['port']}"
    finally:
        run.stop()


def test_product_engine_reads_no_key_files_until_the_app_hands_keys(product, fakes):
    run, url = product
    assert call(url + "/ai/credentials", "POST", {"typesafe": None, "openrouter": None}) == (
        200, {"typesafe": False, "openrouter": False})
    assert call(url + "/health")[1]["jev"] is None
    keys = call(url + "/config")[1]["keys"]
    assert [(k["name"], k["state"], k["in_app"]) for k in keys] == [
        ("openrouter", "missing", False), ("typesafe", "missing", False), ("slack", "missing", False)]
    status, out = call(url + "/ask", "POST", {"text": QUESTION})
    assert status == 200 and list(out) == ASK_DOWN_KEYS and out["error_kind"] == "key"
    status, out = call(url + "/answer", "POST", {"text": QUESTION, "chunk_ids": [0]})
    assert status == 200 and out["error_kind"] == "key"
    assert fakes.jev.seen() == [] and fakes.openrouter.seen() == []

    assert call(url + "/ai/credentials", "POST", {"typesafe": APP_TS, "openrouter": APP_OR}) == (
        200, {"typesafe": True, "openrouter": True})
    keys = call(url + "/config")[1]["keys"]
    assert [(k["name"], k["state"], k["in_app"]) for k in keys][:2] == [("openrouter", "ok", True),
                                                                        ("typesafe", "ok", True)]
    status, out = call(url + "/ask", "POST", {"text": QUESTION})
    assert status == 200 and "error" not in out and out["shown"] is True
    status, out = call(url + "/answer", "POST", {"text": QUESTION, "chunk_ids": [0]})
    assert status == 200 and out["answer"]
    assert {r["auth"] for r in fakes.jev.seen()} == {f"Bearer {APP_TS}"}
    assert {r["auth"] for r in fakes.openrouter.seen()} == {f"Bearer {APP_OR}"}

    # removing the app's key: no file to fall back on in a product engine
    assert call(url + "/ai/credentials", "POST", {"typesafe": None}) == (200, {"typesafe": False, "openrouter": True})
    assert call(url + "/health")[1]["jev"] == "openrouter"
    assert call(url + "/ai/credentials", "POST", {"openrouter": ""}) == (200, {"typesafe": False, "openrouter": False})
    assert call(url + "/ask", "POST", {"text": QUESTION})[1]["error_kind"] == "key"

    status, out = call(url + "/ai/credentials", "POST", {"typesafe": "bad key " + APP_TS})
    assert status == 400 and APP_TS not in out["error"]
    assert not [line for line in run.stderr if APP_TS in line or APP_OR in line]
