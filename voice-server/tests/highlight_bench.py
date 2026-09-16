"""Policy highlights against model highlights, on real replies.

    OPENROUTER_API_KEY=... python -m tests.highlight_bench [--models m1,m2] [--extra transcript.jsonl ...]

Corpus: every `assistant` line in the mock kernel transcripts the harness left under tests/out/,
plus any transcript.jsonl given. For each reply: the policy line (`narrator.highlight`), and each
model's line through the same guardrails (`guard_highlight`). Measures per model: latency, cost
(OpenRouter's usage.cost), length, how often the guardrails rejected it, and whether the line kept
the reply's numbers and names. Writes tests/out/highlight-bench.md and prints it.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import sys
import time
from pathlib import Path

import httpx

from voice_server.narrator import HIGHLIGHT_PROMPT, guard_highlight, highlight

HERE = Path(__file__).resolve().parent
OUT = HERE / "out"


def corpus(extra: list[str]) -> list[str]:
    seen: list[str] = []
    files = sorted(OUT.glob("*/place/.arbos/agents/*/transcript.jsonl")) + [Path(p) for p in extra]
    for f in files:
        if not f.exists():
            continue
        for line in f.read_text().splitlines():
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            if ev.get("kind") == "assistant" and len(str(ev.get("text", "")).strip()) > 20:
                text = str(ev["text"]).strip()
                if text not in seen:
                    seen.append(text)
    return seen


async def ask(model: str, key: str, reply: str) -> tuple[str, float, float]:
    body = {
        "model": model,
        "messages": [{"role": "user", "content": HIGHLIGHT_PROMPT.format(screen="on your screen", reply=reply[:6000])}],
        "max_tokens": 120,
        "temperature": 0.2,
        "usage": {"include": True},
    }
    t0 = time.monotonic()
    async with httpx.AsyncClient(timeout=20.0) as client:
        r = await client.post("https://openrouter.ai/api/v1/chat/completions", json=body,
                              headers={"Authorization": f"Bearer {key}", "X-Title": "Arbos narrator bench"})
        r.raise_for_status()
        data = r.json()
    ms = (time.monotonic() - t0) * 1000
    cost = float((data.get("usage") or {}).get("cost") or 0.0)
    return str(data["choices"][0]["message"]["content"]).strip(), ms, cost


_TOKENS = re.compile(r"\d+(?:[.,]\d+)?|[A-Z][a-zA-Z0-9_-]{2,}|[a-z]+-[a-z-]+")


def kept(line: str, reply: str) -> float:
    """Share of the reply's numbers, capitalised names and slugs (fix-skeptic-test) that the line kept —
    the things a caller would ask about. 1.0 when the reply has none."""
    keys = set(_TOKENS.findall(reply)) - {"The", "This", "That", "Next", "Here"}
    if not keys:
        return 1.0
    low = line.lower()
    return sum(1 for k in keys if k.lower() in low) / len(keys)


async def main_async(opts: argparse.Namespace) -> int:
    key = os.environ.get("OPENROUTER_API_KEY") or os.environ.get("OPENROUTER")
    if not key:
        print("OPENROUTER_API_KEY is not set", file=sys.stderr)
        return 2
    replies = corpus(opts.extra)
    if not replies:
        print("no replies found; run the harness first", file=sys.stderr)
        return 2
    models = [m for m in opts.models.split(",") if m]
    rows: list[dict] = []
    for reply in replies:
        row = {"reply": reply, "policy": highlight(reply), "models": {}}
        for m in models:
            try:
                raw, ms, cost = await ask(m, key, reply)
            except Exception as exc:
                row["models"][m] = {"raw": f"(error: {exc})", "ok": None, "ms": 0, "cost": 0}
                continue
            checked = guard_highlight(raw, reply, row["policy"])
            row["models"][m] = {"raw": raw, "ok": checked, "ms": ms, "cost": cost}
        rows.append(row)

    lines = ["# Highlight bench: policy vs narrator model", "", f"{len(replies)} replies. Guardrails: length ≤ 240, no code/links/lists, no numbers absent from the reply.", ""]
    lines.append("| variant | mean chars | kept names/numbers | guard rejects | mean latency | cost / highlight |")
    lines.append("|---|---|---|---|---|---|")
    pol = [r["policy"] for r in rows]
    lines.append(f"| policy | {sum(len(p) for p in pol) / len(pol):.0f} | {sum(kept(p, r['reply']) for p, r in zip(pol, rows)) / len(rows):.0%} | 0 | 0 ms | $0 |")
    for m in models:
        got = [r["models"][m] for r in rows]
        oks = [g for g in got if g["ok"]]
        rejects = sum(1 for g in got if g["ok"] is None)
        chars = sum(len(g["ok"]) for g in oks) / max(1, len(oks))
        kp = sum(kept(g["ok"], r["reply"]) for g, r in zip(got, rows) if g["ok"]) / max(1, len(oks))
        ms = sum(g["ms"] for g in got) / max(1, len(got))
        cost = sum(g["cost"] for g in got) / max(1, len(got))
        lines.append(f"| {m} | {chars:.0f} | {kp:.0%} | {rejects}/{len(got)} | {ms:.0f} ms | ${cost:.5f} |")
    lines += ["", "## Lines", ""]
    for r in rows:
        lines.append(f"**Reply** ({len(r['reply'])} chars): {r['reply'][:300].replace(chr(10), ' ')}{'…' if len(r['reply']) > 300 else ''}")
        lines.append(f"- policy: {r['policy']}")
        for m in models:
            g = r["models"][m]
            verdict = "ok" if g["ok"] else "REJECTED → policy"
            lines.append(f"- {m} ({g['ms']:.0f} ms, {verdict}): {g['raw'].replace(chr(10), ' ')}")
        lines.append("")
    text = "\n".join(lines)
    (OUT / "highlight-bench.md").write_text(text)
    print(text)
    return 0


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--models", default="google/gemini-2.5-flash-lite,google/gemini-2.5-flash")
    p.add_argument("--extra", nargs="*", default=[], help="more transcript.jsonl files to draw replies from")
    raise SystemExit(asyncio.run(main_async(p.parse_args())))


if __name__ == "__main__":
    main()
