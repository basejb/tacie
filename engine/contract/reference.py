"""The reference the contract compares the engine with: the deleted Python engine's card() (engine/cards.py) and model
defaults (engine/settings.py DEFAULTS), copied as they were when it was deleted (2026-10-26). The app reads cards and
/config in exactly this shape, so this copy stays fixed; a deliberate contract change edits it here and in Rust.
"""
import hashlib
import re
from pathlib import Path

DEFAULTS = {"stt_model": "elevenlabs/scribe-v2", "answer_model": "anthropic/claude-haiku-5.5", "gate": 0.7, "show": 0.8,
            "end": 0.8}
HEADER = re.compile(r"^(file|symbols|doc|section|github|slack|meta|files): ?(.*)$")
SNIPPET_LINES = 12


def split_chunk(text):
    """Chunk text = header lines (file:/symbols:/doc:/section:, github:/slack:/meta:) then the body.
    Returns (header dict, body lines)."""
    lines = text.split("\n")
    head = {}
    while lines and (m := HEADER.match(lines[0])):
        head[m.group(1)] = m.group(2)
        lines = lines[1:]
    return head, lines


def title_of(chunk, head):
    if chunk.get("title"):
        return chunk["title"]
    name = Path(chunk["path"]).name
    if chunk["src"] == "docs":
        section = head.get("section", "").split(" > ")[-1].strip()
        return f"{name} · {section}" if section else name
    symbol = head.get("symbols", "").split(",")[0].strip()
    return f"{name} · {symbol}" if symbol else name


def card(chunk, score, meta, repo_dir=None, chunk_id=None):
    head, body = split_chunk(chunk["text"])
    start = chunk["line"]
    end = start + max(len(body), 1) - 1
    in_repo = chunk["src"] in ("code", "docs")
    c = {
        "id": "c_" + hashlib.sha1(f"{chunk['path']}:{start}".encode()).hexdigest()[:8],
        "path": chunk["path"],
        "kind": chunk["src"],
        "source": "repo" if in_repo else chunk["src"],
        "line_start": start,
        "line_end": end,
        "title": title_of(chunk, head),
        # where it came from, in words: "머지됨 · basejb · 2026-10-15 · …" or "#dev-help · 2026-10-21 · 스레드 …"
        "byline": None if in_repo else " · ".join(x for x in (head.get("slack"), head.get("meta")) if x),
        "snippet": "\n".join(body[:SNIPPET_LINES]).strip("\n"),
        "score": score,
        "url": chunk.get("url"),
        "github_url": None,
        "local_path": str(Path(repo_dir) / chunk["path"]) if repo_dir and in_repo else None,
        "chunk": chunk_id,
    }
    if in_repo and meta.get("repo") and meta.get("commit"):
        c["url"] = c["github_url"] = f"https://github.com/{meta['repo']}/blob/{meta['commit']}/{chunk['path']}#L{start}-L{end}"
    elif chunk["src"] == "github":
        c["github_url"] = c["url"]
    return c
