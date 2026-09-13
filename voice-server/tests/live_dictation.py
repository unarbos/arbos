"""Dictation latency and accuracy against a running gateway, on a fixed sentence.

    VOICE_TOKEN=... python -m tests.live_dictation --url wss://HOST/ws [--runs 3]

Streams a Kokoro utterance of the sentence at real time twice per run: once as a `dictation`
session (the ASR pipeline: faster-whisper partials, a final on end of speech) and once as a
plain `voice` session (the old Fn path: the duplex speech model's transcript, no partials).
Measures first partial and final latency from the end of the utterance, and word error rate.
"""

from __future__ import annotations

import argparse
import asyncio
import os
import sys
import time

from tests import speech
from tests.client import Caller
from tests.desktop_dictation import SENTENCE, wer


async def one(url: str, token: str, mode: str, pcm: bytes) -> dict:
    c = Caller(url, token=token, mode=mode, device="desktop")
    r = await c.connect(timeout=30)
    await asyncio.sleep(0.8)
    start = c.now()
    await c.say(pcm, SENTENCE)
    end = c.now()
    try:
        await c.wait_frame("transcript.final", 1, timeout=20)
    except TimeoutError:
        pass
    await asyncio.sleep(0.3)
    deltas = [f for f in c.rec.of("transcript.delta")]
    finals = [f for f in c.rec.of("transcript.final") if f.msg.get("text", "").strip()]
    await c.close()
    text = finals[0].msg.get("text", "") if finals else ""
    return {
        "mode": mode,
        "engine": r.get("engine"),
        "asr": r.get("asr"),
        "first_partial_s": round(deltas[0].at - start, 2) if deltas else None,
        "partials": len(deltas),
        "final_after_end_s": round(finals[0].at - end, 2) if finals else None,
        "text": text,
        "wer": round(wer(SENTENCE, text), 3) if text else 1.0,
    }


async def main_async(opts: argparse.Namespace) -> int:
    token = os.environ.get("VOICE_TOKEN", "")
    if not token or not speech.kokoro_available():
        print("needs VOICE_TOKEN and the Kokoro weights", file=sys.stderr)
        return 2
    pcm = speech.utterance(SENTENCE, prefer="kokoro")
    print(f"sentence: {SENTENCE!r} ({speech.seconds(pcm):.1f} s of Kokoro speech)")
    rows = []
    for _ in range(opts.runs):
        for mode in ("dictation", "voice"):
            rows.append(await one(opts.url, token, mode, pcm))
            print(rows[-1])
            await asyncio.sleep(1.0)
    print("\n| mode | engine/asr | first partial (s from start) | final (s after end) | WER | text |")
    print("|---|---|---|---|---|---|")
    for mode in ("dictation", "voice"):
        got = [r for r in rows if r["mode"] == mode]
        fp = [r["first_partial_s"] for r in got if r["first_partial_s"] is not None]
        fa = [r["final_after_end_s"] for r in got if r["final_after_end_s"] is not None]
        w = [r["wer"] for r in got]
        print(f"| {mode} | {got[0]['engine']}/{got[0]['asr']} | {sum(fp)/len(fp):.2f} ({len(fp)}/{len(got)} had partials) | {sum(fa)/len(fa):.2f} | {sum(w)/len(w):.0%} | {got[-1]['text']!r} |" if fa else f"| {mode} | – | – | no final | 100% | |")
    return 0


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--url", required=True)
    p.add_argument("--runs", type=int, default=3)
    raise SystemExit(asyncio.run(main_async(p.parse_args())))


if __name__ == "__main__":
    main()
