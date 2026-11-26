"""The engine's HTTP contract for the local MCP tools: /tools/meetings, /tools/ledger and /tools/search."""
from .test_core import QUESTION, call

SEARCH_KEYS = ["cards", "confident", "ranked"]
CARD_KEYS = ["title", "source", "url", "snippet", "score"]


def test_tools_need_the_token(engine):
    for path in ("/tools/meetings", "/tools/ledger", "/tools/search"):
        assert call(engine.url + path, "POST", {}, token=None) == (401, {"error": "unauthorized"}), path
        assert call(engine.url + path, "POST", {}, token="wrong") == (401, {"error": "unauthorized"}), path


def test_tools_search_shape(engine):
    status, out = call(engine.url + "/tools/search", "POST", {"text": QUESTION})
    assert status == 200
    assert list(out) == SEARCH_KEYS
    assert out["ranked"] is True and out["confident"] is True
    assert 0 < len(out["cards"]) <= 8
    for c in out["cards"]:
        assert list(c)[:len(CARD_KEYS)] == CARD_KEYS and len(c["snippet"]) <= 600
    assert call(engine.url + "/tools/search", "POST", {"text": " "}) == (400, {"error": "text is required"})
    assert call(engine.url + "/tools/search", "POST", {"text": "x", "meeting_ids": "a"})[0] == 400


def test_tools_meetings_and_ledger_shape(sources_engine):
    status, out = call(sources_engine.url + "/tools/meetings", "POST", {})
    assert status == 200 and list(out) == ["meetings", "more"]
    status, out = call(sources_engine.url + "/tools/ledger", "POST", {})
    assert status == 200 and list(out) == ["entries", "more"]
