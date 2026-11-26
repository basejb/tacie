"""The engine's HTTP contract for meeting sessions.

Utterances come in as text (/utterances) or as WAV (/audio, through the fake STT, which picks its transcript by the
WAV's byte length). The fake Jev's gate says p_lookup and p_end per test (gate_plan, end_plan); the fake ledger model
answers items on the utterance ids it was sent. As in test_core, the fixture's vectors are fake: these tests check
shapes, key order, ids, joining, repeats and what reaches the fakes, never which chunks a question finds.
"""
import base64
import json
import re
import urllib.error
import urllib.request

from .fakes import (ANSWER, JEV_PATH, LEDGER_COMMITMENT, LEDGER_DECISION, LEDGER_DUE, LEDGER_OPEN, LEDGER_SUMMARY,
                    LEDGER_TITLE, OPENROUTER_BASE, STT_BY_LEN, STT_CONFIDENCE, wav_bytes)
from .reference import DEFAULTS, card
from .test_core import CARD_KEYS, SEARCHABLE, call

WAV = {text: wav_bytes(n) for n, text in STT_BY_LEN.items()}
SILENCE = wav_bytes(3300)  # a length the fake STT has no text for: an empty transcript
FIRST_PIECE, SECOND_PIECE, QUESTION = STT_BY_LEN.values()
CHAT = "점심 뭐 먹을까요?"
ENDING = "오늘은 여기까지 할게요"
GATE_PLAN = {"어디서": 0.9}  # the gate fires on lines with 어디서; every other line gets REST (0.1)
END_PLAN = {"여기까지": 0.9}

BACKCHANNEL_KEYS = ["utterance_id", "text", "speaker", "voice", "person", "at", "gap_ms", "cards", "shown", "merged_from",
                    "skipped", "p_lookup", "fired", "p_end", "ending", "stt_confidence"]
HEARD_KEYS = ["utterance_id", "text", "speaker", "voice", "person", "at", "gap_ms", "stt_confidence", "cards", "shown",
              "merged_from", "p_lookup", "fired", "p_end", "ending", "timing_ms"]
FOUND_KEYS = HEARD_KEYS + ["best_p", "low_confidence", "cost_usd"]
GATE_DOWN_KEYS = HEARD_KEYS[:-1] + ["error", "error_kind"]
SESSION_CARD_KEYS = CARD_KEYS + ["repeat"]
SUMMARY_KEYS = ["session_id", "utterances", "fired", "answered", "unanswered"]
UNANSWERED_KEYS = ["utterance_id", "text", "best_p"]
NO_TRANSCRIPT_KEYS = ["utterance_id", "text", "fired", "timing_ms", "stt"]
STT_DOWN_KEYS = ["utterance_id", "text", "fired", "error", "error_kind", "stt"]
NOTES_KEYS = ["kind", "kind_source", "title", "summary", "topics", "items", "dropped", "ms", "cost"]
ITEM_KEYS = {"decision": ["kind", "text", "utterance_ids"], "open_question": ["kind", "text", "utterance_ids"],
             "commitment": ["kind", "text", "utterance_ids", "owner", "due"]}
NO_SESSION = "s99999"
STT_PATH = OPENROUTER_BASE + "/audio/transcriptions"
CHAT_PATH = OPENROUTER_BASE + "/chat/completions"


def new_session(engine):
    status, out = call(engine.url + "/sessions", "POST", {})
    assert status == 200 and list(out) == ["session_id"]
    assert re.fullmatch(r"s\d+", out["session_id"]), out
    return out["session_id"]


def say(engine, sid, text, speaker="나", **extra):
    """One utterance as text, the way the ask window's meeting mode sends it."""
    return call(f"{engine.url}/sessions/{sid}/utterances", "POST", {"text": text, "speaker": speaker, **extra})


def hear(engine, sid, wav, gap_ms=None, speaker=None):
    """One utterance as WAV, the way the app sends a recorded piece: (status, parsed JSON)."""
    headers = {"Authorization": "Bearer contract", "Content-Type": "audio/wav"}
    if gap_ms is not None:
        headers["X-Gap-Ms"] = str(gap_ms)
    if speaker is not None:
        headers["X-Speaker"] = speaker
    req = urllib.request.Request(f"{engine.url}/sessions/{sid}/audio", method="POST", data=wav, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read())


def cards_of(engine, sid, uid, body=None):
    return call(f"{engine.url}/sessions/{sid}/utterances/{uid}/cards", "POST", {} if body is None else body)


def gates(engine):
    """The state of every gate request Jev got, in order."""
    return [r["body"]["state"] for r in engine.jev.seen() if "lookup" in r["body"]["questions"]]


def check_session_cards(engine, cards, shown_before):
    """1–3 cards, one per document, each reference.card() of its chunk plus `repeat`: shown in this session already."""
    assert 0 < len(cards) <= 3 and len({c["path"] for c in cards}) == len(cards)
    for c in cards:
        assert list(c) == SESSION_CARD_KEYS
        assert c["chunk"] in SEARCHABLE
        plain = {k: v for k, v in c.items() if k != "repeat"}
        assert plain == card(engine.chunks[c["chunk"]], c["score"], engine.meta, None, c["chunk"])
        assert c["repeat"] is (c["path"] in shown_before), c["path"]
    assert [c["score"] for c in cards] == [0.9, 0.1, 0.1][:len(cards)]


def test_session_create_summary_delete(engine):
    engine.jev.gate_plan = dict(GATE_PLAN)
    sid = new_session(engine)
    other = new_session(engine)
    assert int(other[1:]) > int(sid[1:])
    status, out = call(f"{engine.url}/sessions/{sid}")
    assert status == 200 and list(out) == SUMMARY_KEYS
    assert out == {"session_id": sid, "utterances": 0, "fired": 0, "answered": [], "unanswered": []}
    assert engine.jev.seen() == [] and engine.openrouter.seen() == []  # a session costs nothing until it hears

    status, answered = say(engine, sid, QUESTION)
    assert status == 200 and answered["fired"] is True and answered["shown"] is True
    status, unanswered = say(engine, sid, "포트는 어디서 정해요?", show_threshold=0.95)
    assert status == 200 and unanswered["fired"] is True and unanswered["shown"] is False
    assert say(engine, sid, "네")[1]["skipped"] == "backchannel"
    assert say(engine, sid, CHAT)[1]["fired"] is False

    status, out = call(f"{engine.url}/sessions/{sid}")
    assert status == 200 and list(out) == SUMMARY_KEYS
    assert (out["session_id"], out["utterances"], out["fired"]) == (sid, 4, 2)
    assert out["answered"] == [answered]  # the whole record, as the utterance call returned it
    assert [list(u) for u in out["unanswered"]] == [UNANSWERED_KEYS]
    assert out["unanswered"] == [{"utterance_id": f"{sid}u2", "text": "포트는 어디서 정해요?", "best_p": 0.9}]
    assert call(f"{engine.url}/sessions/{other}")[1]["utterances"] == 0  # sessions keep their own lines

    assert call(f"{engine.url}/sessions/{sid}", "DELETE") == (200, {"deleted": sid})
    gone = (404, {"error": "not found"})
    assert call(f"{engine.url}/sessions/{sid}") == gone
    assert call(f"{engine.url}/sessions/{sid}", "DELETE") == gone
    assert say(engine, sid, QUESTION) == gone
    assert cards_of(engine, sid, f"{sid}u1") == gone
    assert call(f"{engine.url}/sessions/{NO_SESSION}") == gone
    third = new_session(engine)
    assert third not in (sid, other)  # a deleted id is not given out again
    for s in (other, third):
        assert call(f"{engine.url}/sessions/{s}", "DELETE") == (200, {"deleted": s})


def test_voices_route_is_empty_without_model(engine):
    """The contract engine has no voice model: no utterance gets a voice, so a session has no voices to list."""
    sid = new_session(engine)
    assert say(engine, sid, CHAT)[1]["person"] is None
    assert call(f"{engine.url}/sessions/{sid}/voices", "POST", {}) == (200, {"voices": []})
    assert call(f"{engine.url}/sessions/{sid}/voices", "POST", {"exclude": [f"{sid}u1"]}) == (200, {"voices": []})
    assert call(f"{engine.url}/sessions/{NO_SESSION}/voices", "POST", {}) == (404, {"error": "no such session"})
    bad = {"people": [{"id": "p1", "vector": [0.1] * 3}]}
    assert call(engine.url + "/sessions", "POST", bad) == (400, {"error": "people and restore must be lists of voices"})
    restore = [{"voice": "화자 2", "channel": "call", "vector": [0.1] * 512, "seconds": 3.0}]
    status, out = call(engine.url + "/sessions", "POST", {"people": [], "restore": restore})
    assert status == 200 and list(out) == ["session_id"]
    assert call(f"{engine.url}/sessions/{out['session_id']}/voices", "POST", {})[1]["voices"][0]["voice"] == "화자 2"
    # `diar_vector` (the speaker-grouping model's voice) is optional; without that model the engine keeps `vector` only
    with_diar = [dict(restore[0], diar_vector=[0.1] * 192)]
    status, kept = call(engine.url + "/sessions", "POST", {"restore": with_diar})
    assert status == 200
    v = call(f"{engine.url}/sessions/{kept['session_id']}/voices", "POST", {})[1]["voices"][0]
    assert v["voice"] == "화자 2" and "diar_vector" not in v
    for bad_diar in ([], ["x"], 3):
        bad = {"restore": [dict(restore[0], diar_vector=bad_diar)]}
        assert call(engine.url + "/sessions", "POST", bad) == (400, {"error": "people and restore must be lists of voices"})
    for s in (sid, out["session_id"], kept["session_id"]):
        assert call(f"{engine.url}/sessions/{s}", "DELETE") == (200, {"deleted": s})


def test_utterance_backchannel_and_fired_shapes(engine):
    engine.jev.gate_plan, engine.jev.end_plan = dict(GATE_PLAN), dict(END_PLAN)
    sid = new_session(engine)

    status, out = say(engine, sid, "네네.")
    assert status == 200 and list(out) == BACKCHANNEL_KEYS
    assert {k: v for k, v in out.items() if k != "at"} == {
        "utterance_id": f"{sid}u1", "text": "네네.", "speaker": "나", "voice": None, "person": None, "gap_ms": None, "cards": [], "shown": False,
        "merged_from": None, "skipped": "backchannel", "p_lookup": None, "fired": False, "p_end": None,
        "ending": False, "stt_confidence": None}
    assert isinstance(out["at"], float)
    assert engine.jev.seen() == []  # a backchannel is not gated

    status, out = say(engine, sid, CHAT)
    assert status == 200 and list(out) == HEARD_KEYS
    assert out["utterance_id"] == f"{sid}u2" and out["text"] == CHAT and out["speaker"] == "나"
    assert (out["p_lookup"], out["fired"], out["p_end"], out["ending"]) == (0.1, False, 0.1, False)
    assert out["cards"] == [] and out["shown"] is False and out["merged_from"] is None
    assert out["gap_ms"] is None and out["stt_confidence"] is None
    assert list(out["timing_ms"]) == ["gate"]

    status, out = say(engine, sid, QUESTION)
    assert status == 200 and list(out) == FOUND_KEYS
    assert out["utterance_id"] == f"{sid}u3" and out["text"] == QUESTION
    assert (out["p_lookup"], out["fired"], out["p_end"], out["ending"]) == (0.9, True, 0.1, False)
    assert out["best_p"] == 0.9 and out["shown"] is True and out["low_confidence"] is False
    assert out["cost_usd"] > 0
    assert list(out["timing_ms"]) == ["gate", "search", "rerank"]
    check_session_cards(engine, out["cards"], shown_before=set())

    status, out = say(engine, sid, ENDING)
    assert status == 200 and list(out) == HEARD_KEYS
    assert (out["p_lookup"], out["fired"], out["p_end"], out["ending"]) == (0.1, False, 0.9, True)

    # what the gate was asked: the backchannel is no one's context; each line sees the ones before it
    assert gates(engine) == [
        {"recent_conversation": "(start of meeting)", "current_utterance": f"나: {CHAT}"},
        {"recent_conversation": f"나: {CHAT}", "current_utterance": f"나: {QUESTION}"},
        {"recent_conversation": f"나: {CHAT}\n나: {QUESTION}", "current_utterance": f"나: {ENDING}"},
    ]
    gate_req = next(r for r in engine.jev.seen() if "lookup" in r["body"]["questions"])
    assert gate_req["body"]["model"] == "jev-latest"
    assert list(gate_req["body"]["questions"]) == ["lookup", "source", "end"]
    (rerank,) = [r["body"]["state"] for r in engine.jev.seen() if "passages" in r["body"]["state"]]
    assert rerank["recent_conversation"] == f"나: {CHAT}" and rerank["current_message"] == f"나: {QUESTION}"

    engine.jev.mode = "down"  # the gate fails: the line is kept, marked with why
    status, out = say(engine, sid, QUESTION)
    assert status == 200 and list(out) == GATE_DOWN_KEYS
    assert (out["p_lookup"], out["fired"], out["p_end"], out["ending"]) == (None, False, None, False)
    assert out["error_kind"] == "down" and isinstance(out["error"], str) and out["error"]
    assert out["utterance_id"] == f"{sid}u5"


def test_short_pause_join_through_api(engine):
    engine.jev.gate_plan = dict(GATE_PLAN)
    sid = new_session(engine)

    status, first = hear(engine, sid, WAV[FIRST_PIECE], gap_ms=3000, speaker="me")
    assert status == 200 and list(first) == HEARD_KEYS + ["stt", "words"]
    assert first["utterance_id"] == f"{sid}u1" and first["text"] == FIRST_PIECE and first["speaker"] == "나"
    assert first["gap_ms"] == 3000 and first["merged_from"] is None and first["fired"] is False
    assert first["stt_confidence"] == STT_CONFIDENCE
    assert list(first["timing_ms"]) == ["stt", "gate"]

    status, second = hear(engine, sid, WAV[SECOND_PIECE], gap_ms=800, speaker="me")
    assert status == 200 and list(second) == HEARD_KEYS + ["stt", "words"]
    joined = f"{FIRST_PIECE} {SECOND_PIECE}"
    assert second["utterance_id"] == f"{sid}u2" and second["merged_from"] == f"{sid}u1"
    assert second["text"] == joined and second["gap_ms"] == 800
    assert second["p_lookup"] == 0.9 and second["fired"] is True
    assert second["cards"] == [] and second["shown"] is False  # /audio only hears; the app asks for cards

    # the other side, then me after a short pause: a different speaker is not joined
    status, third = hear(engine, sid, WAV[FIRST_PIECE], gap_ms=3000, speaker="them")
    assert status == 200 and third["speaker"] == "상대" and third["merged_from"] is None
    status, fourth = hear(engine, sid, WAV[SECOND_PIECE], gap_ms=800, speaker="me")
    assert status == 200 and fourth["merged_from"] is None and fourth["text"] == SECOND_PIECE

    assert gates(engine) == [
        {"recent_conversation": "(start of meeting)", "current_utterance": f"나: {FIRST_PIECE}"},
        {"recent_conversation": "(start of meeting)", "current_utterance": f"나: {joined}"},
        {"recent_conversation": f"나: {joined}", "current_utterance": f"상대: {FIRST_PIECE}"},
        {"recent_conversation": f"나: {joined}\n상대: {FIRST_PIECE}", "current_utterance": f"나: {SECOND_PIECE}"},
    ]

    status, found = cards_of(engine, sid, second["utterance_id"])
    assert status == 200 and list(found) == FOUND_KEYS
    assert found["text"] == joined and found["merged_from"] == f"{sid}u1"
    assert list(found["timing_ms"]) == ["gate", "search", "rerank"]  # the kept record has no stt time

    stt = [r for r in engine.openrouter.seen() if r["path"] == STT_PATH]
    assert len(stt) == 4
    body = stt[0]["body"]
    assert base64.b64decode(body["input_audio"]["data"]) == WAV[FIRST_PIECE]
    assert {k: v for k, v in body.items() if k != "input_audio"} == {
        "model": DEFAULTS["stt_model"], "language": "ko", "provider": {"zdr": True},
        "response_format": "verbose_json", "timestamp_granularities": ["word"]}
    assert body["input_audio"]["format"] == "wav"


def test_audio_words(engine):
    sid = new_session(engine)
    status, out = hear(engine, sid, WAV[QUESTION], speaker="me")
    assert status == 200 and list(out)[-1] == "words"
    assert list(out["words"][0]) == ["text", "start", "end"]
    assert out["words"][0] == {"text": "링크", "start": 0.0, "end": 0.4}


def test_audio_errors(engine):
    refused = (400, {"error": "send one utterance as WAV, up to 30 s"})
    no_session = (404, {"error": "no such session"})
    assert hear(engine, NO_SESSION, WAV[QUESTION]) == no_session
    assert hear(engine, NO_SESSION, b"") == no_session  # the session is checked before the body
    sid = new_session(engine)
    assert hear(engine, sid, b"") == refused
    assert engine.openrouter.seen() == [] and engine.jev.seen() == []

    status, out = hear(engine, sid, SILENCE, gap_ms=500, speaker="me")  # nothing said: nothing logged
    assert status == 200 and list(out) == NO_TRANSCRIPT_KEYS
    assert {k: v for k, v in out.items() if k != "timing_ms"} == {"utterance_id": None, "text": "", "fired": False, "stt": "server"}
    assert list(out["timing_ms"]) == ["stt"]

    engine.openrouter.mode = "down"
    status, out = hear(engine, sid, WAV[QUESTION], speaker="me")
    assert status == 200 and list(out) == STT_DOWN_KEYS
    assert (out["utterance_id"], out["text"], out["fired"], out["error_kind"]) == (None, "", False, "down")
    assert isinstance(out["error"], str) and out["error"]

    assert call(f"{engine.url}/sessions/{sid}")[1]["utterances"] == 0
    assert engine.jev.seen() == []  # no transcript, no gate
    assert [r["path"] for r in engine.openrouter.seen()] == [STT_PATH, STT_PATH]


def test_cards_route_and_repeat(engine):
    engine.jev.gate_plan = dict(GATE_PLAN)
    sid = new_session(engine)
    status, first = say(engine, sid, QUESTION)
    assert status == 200 and first["fired"] is True and first["shown"] is True
    check_session_cards(engine, first["cards"], shown_before=set())
    shown = {c["path"] for c in first["cards"]}

    # the same utterance again: every card it showed is a repeat now
    status, again = cards_of(engine, sid, first["utterance_id"])
    assert status == 200 and list(again) == FOUND_KEYS
    assert [c["path"] for c in again["cards"]] == [c["path"] for c in first["cards"]]
    check_session_cards(engine, again["cards"], shown_before=shown)
    assert all(c["repeat"] for c in again["cards"])

    # a line the gate let pass can still be looked up; a higher bar keeps it off the screen
    status, chat = say(engine, sid, CHAT)
    assert status == 200 and chat["fired"] is False
    status, looked = cards_of(engine, sid, chat["utterance_id"], {"show_threshold": 0.95})
    assert status == 200 and list(looked) == FOUND_KEYS
    assert looked["best_p"] == 0.9 and looked["shown"] is False
    check_session_cards(engine, looked["cards"], shown_before=shown)

    status, back = say(engine, sid, "네")
    assert status == 200 and back["skipped"] == "backchannel"
    no_utterance = (404, {"error": "no such utterance"})
    assert cards_of(engine, sid, back["utterance_id"]) == no_utterance  # a backchannel has nothing to look up
    assert cards_of(engine, sid, f"{sid}u99") == no_utterance
    assert cards_of(engine, NO_SESSION, f"{NO_SESSION}u1") == (404, {"error": "not found"})
    url = f"{engine.url}/sessions/{sid}/utterances/{first['utterance_id']}/cards"
    assert call(url, "POST", data=b"{not json") == (400, {"error": "body is not JSON"})

    status, out = call(f"{engine.url}/sessions/{sid}")
    assert (out["utterances"], out["fired"], len(out["answered"]), out["unanswered"]) == (3, 1, 1, [])


def test_names_route_rejects_bad_body(engine):
    sid = new_session(engine)
    url = f"{engine.url}/sessions/{sid}/names"
    assert call(url, "POST", {"names": {"화자 2": "지수"}}) == (200, {})
    assert call(url, "POST", {"names": {}}) == (200, {})
    for bad in [{}, {"names": None}, {"names": ["화자 2"]}, {"names": {"화자 2": 3}}, {"names": "지수"}]:
        assert call(url, "POST", bad) == (400, {"error": "names must map speakers to names"}), bad
    assert call(url, "POST", data=b"{not json") == (400, {"error": "body is not JSON"})
    assert call(f"{engine.url}/sessions/{NO_SESSION}/names", "POST", {"names": {}}) == (404, {"error": "no such session"})


def test_sessions_after_and_restore_limits(engine):
    restore = [{"voice": "화자 2", "channel": "call", "vector": [0.1] * 512, "seconds": 3.0, "person": "p1"}]
    assert call(engine.url + "/sessions", "POST", {"after": 4, "restore": restore})[0] == 200
    bad_voices = (400, {"error": "people and restore must be lists of voices"})
    for bad in [{"after": "4"}, {"after": -1}, {"after": 10001},
                {"restore": [dict(restore[0], voice="화자 10001")]},
                {"restore": [restore[0], dict(restore[0], voice="화자 3")]}]:
        assert call(engine.url + "/sessions", "POST", bad) == bad_voices, bad


def test_session_ledger_and_notes_route(engine):
    engine.jev.gate_plan = dict(GATE_PLAN)
    sid = new_session(engine)
    assert say(engine, sid, "다음 배포는 금요일로 정해요", speaker="상대")[0] == 200
    assert say(engine, sid, "제가 금요일까지 문서 고칠게요")[0] == 200
    assert say(engine, sid, "네")[1]["skipped"] == "backchannel"
    assert say(engine, sid, QUESTION)[1]["fired"] is True
    asked = f"{sid}u4"
    before = len(engine.openrouter.seen())

    status, out = call(f"{engine.url}/sessions/{sid}/ledger", "POST")
    assert status == 200 and list(out) == NOTES_KEYS
    assert out["title"] == LEDGER_TITLE and out["summary"] == [LEDGER_SUMMARY]  # the unsent line is dropped
    assert [list(it) for it in out["items"]] == [ITEM_KEYS["decision"], ITEM_KEYS["commitment"]]
    assert out["items"] == [
        {"kind": "decision", "text": LEDGER_DECISION, "utterance_ids": [f"{sid}u1"]},
        {"kind": "commitment", "text": LEDGER_COMMITMENT, "utterance_ids": [f"{sid}u2"], "owner": "나",
         "due": LEDGER_DUE},
    ]
    assert out["dropped"] == 2  # the unsent id, and the open question that stands only on the asked question
    assert out["cost"] == 0 and out["ms"] >= 0

    assert out["kind"] == "meeting" and out["kind_source"] == "auto"
    # first the recording kind on the transcript's start, then two calls at once: title, summary and topics; and the items
    reqs = engine.openrouter.seen()[before:]
    assert len(reqs) == 3
    assert "<녹음>" in reqs[0]["body"]["messages"][0]["content"] and reqs[0]["body"]["max_tokens"] == 100
    reqs = reqs[1:]
    transcript = (f"[{sid}u1] 상대: 다음 배포는 금요일로 정해요\n[{sid}u2] 나: 제가 금요일까지 문서 고칠게요\n"
                  f"[{asked}] 나: {QUESTION}")
    for req in reqs:
        assert req["path"] == CHAT_PATH
        assert req["body"]["model"] == DEFAULTS["answer_model"] and req["body"]["max_tokens"] == 4000
        prompt = req["body"]["messages"][0]["content"]
        assert prompt.endswith(f"<회의>\n{transcript}\n</회의>")  # no backchannel
        # the contract engine has no voice model: the lines carry the app's 나/상대, which are the attendees and the owner
        assert "\n참석자: 상대, 나\n" in prompt

    # nothing new heard: the same notes, no second call
    assert call(f"{engine.url}/sessions/{sid}/ledger", "POST") == (200, out)
    assert len(engine.openrouter.seen()) == before + 3
    assert call(f"{engine.url}/sessions/{NO_SESSION}/ledger", "POST") == (404, {"error": "no such session"})

    # notes again from the app's own transcript: speakers by me/them, backchannels left out, `asked` drops items
    lines = [{"id": "s7u1", "speaker": "them", "text": "다음 배포는 금요일로 정해요"},
             {"id": "s7u2", "speaker": "me", "text": "제가 금요일까지 문서 고칠게요"},
             {"id": "s7u3", "speaker": "me", "text": "네"},
             {"id": "s7u4", "speaker": None, "text": QUESTION}]
    status, out = call(engine.url + "/ledger", "POST", {"lines": lines})
    assert status == 200 and list(out) == NOTES_KEYS
    assert [list(it) for it in out["items"]] == [ITEM_KEYS["decision"], ITEM_KEYS["commitment"],
                                                 ITEM_KEYS["open_question"]]
    assert out["items"][2] == {"kind": "open_question", "text": LEDGER_OPEN, "utterance_ids": ["s7u4"]}
    assert out["dropped"] == 1
    prompt = engine.openrouter.seen()[-1]["body"]["messages"][0]["content"]
    assert prompt.endswith("<회의>\n[s7u1] 상대: 다음 배포는 금요일로 정해요\n[s7u2] 나: 제가 금요일까지 문서 고칠게요\n"
                           f"[s7u4] 화자: {QUESTION}\n</회의>")
    assert "\n참석자: 상대, 나\n" in prompt  # a line with no speaker is not an attendee

    status, out = call(engine.url + "/ledger", "POST", {"lines": lines, "asked": ["s7u4"]})
    assert status == 200 and list(out) == NOTES_KEYS
    assert [it["kind"] for it in out["items"]] == ["decision", "commitment"] and out["dropped"] == 2

    calls = len(engine.openrouter.seen())
    status, out = call(engine.url + "/ledger", "POST", {"lines": [{"id": "s7u1", "speaker": "me", "text": "네"}]})
    assert status == 200 and list(out) == NOTES_KEYS
    assert out == {"kind": "meeting", "kind_source": "auto", "title": None, "summary": [], "topics": [], "items": [],
                   "dropped": 0, "ms": 0, "cost": 0}
    assert call(engine.url + "/ledger", "POST", {"lines": "s7u1"}) == (
        400, {"error": "lines must be a list of {id, speaker, text}"})
    assert len(engine.openrouter.seen()) == calls  # nothing to read, nothing paid


def test_fakes_saw_only_fake_keys(engine):
    engine.jev.gate_plan = dict(GATE_PLAN)
    sid = new_session(engine)
    status, rec = hear(engine, sid, WAV[QUESTION], gap_ms=2000, speaker="me")
    assert status == 200 and rec["fired"] is True
    assert cards_of(engine, sid, rec["utterance_id"])[1]["shown"] is True
    assert call(f"{engine.url}/sessions/{sid}/ledger", "POST")[0] == 200
    status, out = call(engine.url + "/answer", "POST", {"text": QUESTION, "chunk_ids": [0]})
    assert status == 200 and out["answer"] == ANSWER

    jev, openrouter = engine.jev.seen(), engine.openrouter.seen()
    assert [r["path"] for r in jev] == [JEV_PATH, JEV_PATH]  # gate, rerank
    # stt, recording kind, notes ×2, answer
    assert [r["path"] for r in openrouter] == [STT_PATH, CHAT_PATH, CHAT_PATH, CHAT_PATH, CHAT_PATH]
    assert {r["auth"] for r in jev + openrouter} == {f"Bearer {engine.key}"}
