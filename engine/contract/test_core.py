"""The engine's HTTP contract for /health, /config, /ask and /answer.

The fixture index's vectors are fake, so which chunks a question finds says nothing: these tests check JSON shapes,
key order, counts and rules (thresholds, the access filter, citations, the Jev-down fallback), never which chunks.
The deleted Python engine's card() (reference.py) is the reference every card is compared with.
"""
import json
import urllib.error
import urllib.request

import pytest

from .fakes import ANSWER, INPUT_TOKENS
from .reference import DEFAULTS, card

CARD_KEYS = list(card({"src": "code", "path": "a.py", "line": 1, "text": "x"}, 0.5, {}))
ASK_KEYS = ["cards", "best_p", "shown", "low_confidence", "cost_usd", "timing_ms"]
ASK_DOWN_KEYS = ["cards", "best_p", "shown", "low_confidence", "error", "error_kind", "timing_ms"]
ANSWER_KEYS = ["answer", "ms", "cost", "cited", "dropped"]
# mini: chunk 2 is under docs/superpowers/plans (mask plans), chunk 4 is a private Slack channel (no Slack user here)
SEARCHABLE = [0, 1, 3, 5]
USD_PER_INPUT_TOKEN = 4.2e-8  # engine/jev.py: TypeSafe sends token counts only
QUESTION = "엔진은 포트를 어떻게 알려요?"


def call(url, method="GET", body=None, token="contract", data=None):
    """(status, parsed JSON), as the app sends it. `data` sends raw bytes instead of JSON."""
    if data is None and body is not None:
        data = json.dumps(body, ensure_ascii=False).encode()
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, method=method, data=data, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read())


def check_cards(engine, cards, scores):
    """1–3 cards, one per document, only from searchable chunks, each exactly what reference.card() makes of its
    chunk."""
    assert 0 < len(cards) <= 3
    assert len({c["path"] for c in cards}) == len(cards)
    for c in cards:
        assert list(c) == CARD_KEYS
        assert c["chunk"] in SEARCHABLE
        assert c == card(engine.chunks[c["chunk"]], c["score"], engine.meta, None, c["chunk"])
    assert [c["score"] for c in cards] == scores[:len(cards)]


def test_health_shape(engine):
    assert list(engine.first_line) == ["port", "load_ms"] and isinstance(engine.first_line["load_ms"], int)
    status, out = call(engine.url + "/health")
    assert status == 200
    assert list(out) == ["ready", "jev", "mask", "index"]
    assert list(out["index"]) == ["chunks", "repo", "commit", "built_at"]
    assert out == {"ready": True, "jev": "typesafe", "mask": "plans",
                   "index": {"chunks": 6, "repo": "basejb/tacie", "commit": "abc1234",
                             "built_at": "2026-10-25T00:00:00+09:00"}}


def test_config_shape_and_defaults(engine):
    status, out = call(engine.url + "/config")
    assert status == 200
    assert list(out) == ["models", "model_defaults", "thresholds", "threshold_defaults", "keys", "log_text"]
    assert out["thresholds"] == out["threshold_defaults"] == {"gate": 0.7, "show": 0.8, "end": 0.8}
    assert list(out["thresholds"]) == list(out["threshold_defaults"]) == ["gate", "show", "end"]
    models = {"stt": DEFAULTS["stt_model"], "answer": DEFAULTS["answer_model"], "jev": "jev-latest"}
    assert out["models"] == out["model_defaults"] == models
    assert out["keys"] == [
        {"name": "openrouter", "path": "~/.config/openrouter/config.json", "state": "ok", "in_app": False},
        {"name": "typesafe", "path": "~/.config/typesafe/config.json", "state": "ok", "in_app": False},
        {"name": "slack", "path": "~/.config/slack/config.json", "state": "missing", "in_app": False},
    ]
    assert out["log_text"] is False


def test_unauthorized_and_not_found(engine):
    asks = [("/health", "GET", None), ("/config", "GET", None), ("/ask", "POST", {"text": QUESTION}),
            ("/answer", "POST", {"text": QUESTION, "chunk_ids": [0]}), ("/nope", "GET", None),
            ("/sessions", "POST", {}), ("/sessions/s1/audio", "POST", b"RIFF" + bytes(64)),
            ("/sessions/s1/ledger", "POST", {}), ("/ledger", "POST", {}), ("/questions/review", "POST", {}), ("/sessions/s1", "GET", None)]
    for path, method, body in asks:  # the token comes first: an unknown path without it is 401, not 404
        kw = {"data": body} if isinstance(body, bytes) else {"body": body}
        assert call(engine.url + path, method, token=None, **kw) == (401, {"error": "unauthorized"}), path
        assert call(engine.url + path, method, token="wrong", **kw) == (401, {"error": "unauthorized"}), path
    for path, method in (("/nope", "GET"), ("/nope", "POST"), ("/ask", "GET"), ("/health", "POST")):
        assert call(engine.url + path, method, {} if method == "POST" else None) == (404, {"error": "not found"}), path
    assert engine.jev.seen() == [] and engine.openrouter.seen() == []  # a refused request costs nothing


def test_ask_shape_and_threshold(engine):
    status, out = call(engine.url + "/ask", "POST", {"text": QUESTION})
    assert status == 200
    assert list(out) == ASK_KEYS
    assert out["best_p"] == 0.9 and out["shown"] is True and out["low_confidence"] is False
    check_cards(engine, out["cards"], [0.9, 0.1, 0.1])
    assert out["cost_usd"] == pytest.approx(INPUT_TOKENS * USD_PER_INPUT_TOKEN)
    assert set(out["timing_ms"]) == {"search", "rerank", "total"}
    assert all(isinstance(v, (int, float)) and v >= 0 for v in out["timing_ms"].values())

    # one rerank request for all candidates, with the fake key from HOME; masked and private chunks never reach Jev
    (req,) = engine.jev.seen()
    assert req["auth"] == f"Bearer {engine.key}"
    body = req["body"]
    assert body["model"] == "jev-latest"
    passages = body["state"]["passages"]
    assert list(passages) == list(body["questions"]) == [f"p{k}" for k in range(len(SEARCHABLE))]
    assert sorted(passages.values()) == sorted(engine.chunks[i]["text"] for i in SEARCHABLE)

    engine.jev.mode = "low"
    status, out = call(engine.url + "/ask", "POST", {"text": QUESTION})
    assert status == 200 and list(out) == ASK_KEYS
    assert out["best_p"] == 0.5 and out["shown"] is False and out["low_confidence"] is False
    check_cards(engine, out["cards"], [0.5, 0.1, 0.1])

    engine.jev.mode = "high"  # the ask window may raise the bar
    status, out = call(engine.url + "/ask", "POST", {"text": QUESTION, "show_threshold": 0.95})
    assert status == 200 and out["best_p"] == 0.9 and out["shown"] is False


def test_panel_ask_without_text_is_400(engine):
    assert call(engine.url + "/panel/ask", "POST", {"meeting_ids": ["a"]}) == (400, {"error": "text is required"})
    assert call(engine.url + "/panel/ask", "POST", {"text": "x", "meeting_ids": "a"})[0] == 400


def test_ask_without_text_is_400(engine):
    for body in ({}, {"text": ""}, {"text": "   "}, {"text": None}, {"context": ["나: 안녕"]}):
        assert call(engine.url + "/ask", "POST", body) == (400, {"error": "text is required"}), body
    assert call(engine.url + "/ask", "POST") == (400, {"error": "text is required"})  # no body at all
    assert call(engine.url + "/ask", "POST", data=b"{not json") == (400, {"error": "body is not JSON"})
    assert engine.jev.seen() == []  # no paid call for nothing


def test_ask_when_jev_down(engine):
    engine.jev.mode = "down"
    status, out = call(engine.url + "/ask", "POST", {"text": QUESTION})
    assert status == 200
    assert list(out) == ASK_DOWN_KEYS
    assert out["low_confidence"] is True and out["best_p"] is None and out["error_kind"] == "down"
    assert isinstance(out["error"], str) and out["error"]
    check_cards(engine, out["cards"], [None, None, None])  # dense order, no reranker scores
    assert out["shown"] is True  # the cards still show, marked low confidence
    assert set(out["timing_ms"]) == {"search", "total"}
    assert len(engine.jev.seen()) >= 1


def test_answer_cites_and_filters_ids(engine):
    status, out = call(engine.url + "/answer", "POST", {"text": QUESTION, "chunk_ids": [0, 99, 1]})
    assert status == 200
    assert list(out) == ANSWER_KEYS
    assert out["answer"] == ANSWER and out["answer"].endswith("[1]")
    assert set(out["cited"]) <= {0, 1} and out["cited"] == [0]  # [1] is the first passage left, chunk 0
    assert out["dropped"] is None and out["cost"] == 0

    (req,) = engine.openrouter.seen()  # 99 is not a chunk: two passages go out
    assert req["auth"] == f"Bearer {engine.key}"
    assert req["body"]["model"] == DEFAULTS["answer_model"]
    prompt = req["body"]["messages"][0]["content"]
    assert QUESTION in prompt and prompt.count("<자료 번호=") == 2

    # masked (2), private (4), missing (99) and malformed ids are dropped; nothing left means 400, no call
    required = (400, {"error": "text and chunk_ids are required"})
    assert call(engine.url + "/answer", "POST", {"text": QUESTION, "chunk_ids": [2, 4, 99, -1, "0", 1.5]}) == required
    assert call(engine.url + "/answer", "POST", {"text": "  ", "chunk_ids": [0]}) == required
    assert call(engine.url + "/answer", "POST", {"text": QUESTION}) == required
    assert len(engine.openrouter.seen()) == 1
