"""Fake Jev (TypeSafe decisions) and fake OpenRouter (transcription and chat) for the contract tests.

The engine is pointed here with TACIE_JEV_URL and TACIE_OPENROUTER_URL, so no contract test reaches a paid API.
Each fake is a ThreadingHTTPServer on 127.0.0.1:<free port> in a daemon thread, answers JSON, and keeps every request
it got (path, Authorization header, parsed body) so a test can check what the engine sent.
"""
import base64
import json
import re
import struct
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

JEV_PATH = "/v1/systemone"
OPENROUTER_BASE = "/api/v1"  # the engine appends /chat/completions and /audio/transcriptions
ANSWER = "포트는 stdout 첫 줄로 알려요. [1]"
# p(answers) by mode: passage p0 gets TOP[mode], every other passage REST. "down" answers 500 instead.
TOP = {"high": 0.9, "low": 0.5}
REST = 0.1
INPUT_TOKENS = 1000  # TypeSafe sends token counts only; the engine turns them into cost_usd
# the transcript the fake STT hears in a WAV, chosen by its byte length (wav_bytes builds one); any other length -> ""
STT_BY_LEN = {
    3244: "그 부품들 실제로 어떻게 생겼는지는",
    3246: "어디서 볼 수 있어요?",
    3248: "엔진은 포트를 어디서 알려요?",
}
STT_CONFIDENCE = 0.9  # every word's score; the engine records the mean
# the fake ledger model answers in a code fence, as Haiku often does, with one item and one summary line that point at
# an utterance nobody sent (both must be dropped)
UNSENT_ID = "s999u999"
LEDGER_TITLE = "배포 일정"
LEDGER_DECISION = "다음 배포는 금요일로 정했어요."
LEDGER_COMMITMENT = "금요일까지 문서를 고치기로 했어요."
LEDGER_DUE = "금요일까지"
LEDGER_OPEN = "엔진 포트를 어디서 알리는지 남았어요."
LEDGER_SUMMARY = "배포를 금요일로 정했어요."
_UTTERANCE_ID = re.compile(r"\[(s\d+u\d+)\]")


def wav_bytes(n):
    """A valid 16 kHz mono 16-bit WAV of exactly n bytes: the 44-byte header, then silence."""
    data = n - 44
    assert data >= 0 and data % 2 == 0, n
    return (b"RIFF" + struct.pack("<I", 36 + data) + b"WAVE"
            + b"fmt " + struct.pack("<IHHIIHH", 16, 1, 1, 16000, 32000, 2, 16)
            + b"data" + struct.pack("<I", data) + bytes(data))


class Fake:
    """One fake service. `mode`, `gate_plan`, `end_plan` and `requests` are shared with the tests; `reset()` puts them
    back. The plans matter to Jev's gate only: {substring of current_utterance: p}, the first match wins, else REST."""

    def __init__(self, path, respond):
        self.path, self._respond = path, respond
        self.mode = "high"
        self.gate_plan, self.end_plan = {}, {}
        self.requests = []
        self._lock = threading.Lock()
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), self._handler())
        self.server.daemon_threads = True
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}{path}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def _handler(self):
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, fmt, *args):  # pytest shows stderr of failed tests; keep it for the engine's lines
                pass

            def do_POST(self):
                raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
                try:
                    body = json.loads(raw or b"{}")
                except json.JSONDecodeError:
                    body = None
                with fake._lock:
                    fake.requests.append({"path": self.path, "auth": self.headers.get("Authorization"), "body": body})
                    now = SimpleNamespace(mode=fake.mode, gate_plan=dict(fake.gate_plan), end_plan=dict(fake.end_plan))
                status, out = fake._respond(now, self.path, body)
                data = json.dumps(out, ensure_ascii=False).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        return Handler

    def seen(self):
        with self._lock:
            return list(self.requests)

    def reset(self):
        with self._lock:
            self.mode = "high"
            self.gate_plan, self.end_plan = {}, {}
            self.requests.clear()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


def _planned(plan, utterance):
    return next((p for part, p in plan.items() if part in utterance), REST)


def _jev(now, path, body):
    """Answers each asked question: p{k} -> noul from the mode table, the gate's lookup and end -> the test's
    gate_plan and end_plan (by current_utterance), source -> fixed shares, any other noul -> REST."""
    if now.mode == "down":
        return 500, {"error": "boom"}
    if path != JEV_PATH or not isinstance(body, dict) or not isinstance(body.get("questions"), dict):
        return 404, {"error": "not found"}
    utterance = str((body.get("state") or {}).get("current_utterance") or "")
    answers = {}
    for key in body["questions"]:
        if key == "source":
            answers[key] = {"probabilities": {"code": 0.5, "docs": 0.4, "none": 0.1}}
        elif key == "lookup":
            answers[key] = {"noul": _planned(now.gate_plan, utterance)}
        elif key == "end":
            answers[key] = {"noul": _planned(now.end_plan, utterance)}
        else:
            answers[key] = {"noul": TOP[now.mode] if key == "p0" else REST}
    return 200, {"answers": answers, "usage": {"input_tokens": INPUT_TOKENS}}


# the fake classify reply: a meeting, sure enough to keep the meeting prompts
CLASSIFY_REPLY = '{"kind": "meeting", "confidence": 0.9}'


def _ledger_text(prompt):
    """The ledger reply for the utterance ids in the prompt's <회의> part, in order: a decision on the first, a commitment
    on the second, an open question on the last (third or later), and one item on an id that was never sent."""
    ids = list(dict.fromkeys(_UTTERANCE_ID.findall(prompt.rsplit("<회의>", 1)[-1])))
    items = [{"kind": "decision", "text": "보낸 적 없는 발화예요.", "utterance_ids": [UNSENT_ID]}]
    if ids:
        items.append({"kind": "decision", "text": LEDGER_DECISION, "utterance_ids": ids[:1]})
    if len(ids) >= 2:
        items.append({"kind": "commitment", "text": LEDGER_COMMITMENT, "utterance_ids": [ids[1]],
                      "owner": "나", "due": LEDGER_DUE})
    if len(ids) >= 3:
        items.append({"kind": "open_question", "text": LEDGER_OPEN, "utterance_ids": [ids[-1]]})
    summary = [{"text": LEDGER_SUMMARY, "utterance_ids": ids[:1]},
               {"text": "근거 없는 줄이에요.", "utterance_ids": [UNSENT_ID]}]
    reply = {"title": LEDGER_TITLE, "summary": summary, "items": items}
    return "```json\n" + json.dumps(reply, ensure_ascii=False, indent=2) + "\n```"


def _openrouter(now, path, body):
    """Transcription picks its text by the WAV's byte length (STT_BY_LEN); chat answers the ledger JSON when the prompt
    holds a <회의> transcript, else ANSWER. "down" answers 500 to both."""
    if now.mode == "down":
        return 500, {"error": {"message": "boom"}}
    if not isinstance(body, dict):
        return 404, {"error": "not found"}
    if path == OPENROUTER_BASE + "/audio/transcriptions":
        try:
            size = len(base64.b64decode(body["input_audio"]["data"], validate=True))
        except (KeyError, TypeError, ValueError):
            return 400, {"error": {"message": "input_audio.data is not base64"}}
        return 200, {"text": STT_BY_LEN.get(size, ""), "words": [{"type": "word", "word": "링크", "start": 0.0, "end": 0.4,
                                                       "confidence": STT_CONFIDENCE}],
                     "usage": {"cost": 0}}
    if path != OPENROUTER_BASE + "/chat/completions":
        return 404, {"error": "not found"}
    try:
        prompt = body["messages"][0]["content"]
    except (KeyError, IndexError, TypeError):
        prompt = ""
    if isinstance(prompt, str) and "<녹음>" in prompt:
        content = CLASSIFY_REPLY  # the recording kind, asked before the notes (docs/23)
    elif isinstance(prompt, str) and "<회의>" in prompt:
        content = _ledger_text(prompt)
    else:
        content = ANSWER
    return 200, {"choices": [{"message": {"content": content}, "finish_reason": "stop"}], "usage": {"cost": 0}}


def fake_jev():
    return Fake(JEV_PATH, _jev)


def fake_openrouter():
    return Fake(OPENROUTER_BASE, _openrouter)
