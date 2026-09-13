"""A real call against a running gateway (the pod: real speech model, real kernel), scripted.

    VOICE_TOKEN=... python -m tests.live_call --url wss://HOST/ws [--say "..." ...]

The caller speaks Kokoro-synthesized utterances (needs models/), one after another, waiting for
the narrator between them: the first asks for a sub-agent, then it waits for the highlight and
the sub-agent's report, then asks for detail. Everything the gateway sent is kept under
tests/out/live/: frames.jsonl, transcript.md (the call as a timeline), reply.wav (all reply
audio) and reply-waveform.png (via ffmpeg). No secrets are written: the token stays in the env.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import subprocess
import sys
import time
import wave
from pathlib import Path

from tests import speech
from tests.client import Caller

HERE = Path(__file__).resolve().parent
OUT = HERE / "out" / "live"

DEFAULT_SAYS = [
    "Please send an agent to write a two line haiku about rivers, and report back when it is done.",
    "What exactly did the agent write?",
]


async def main_async(opts: argparse.Namespace) -> int:
    token = os.environ.get("VOICE_TOKEN", "")
    if not token:
        print("VOICE_TOKEN is not set", file=sys.stderr)
        return 2
    if not speech.kokoro_available():
        print(f"no Kokoro weights in {speech.MODELS}; run deploy/fetch-models.sh models", file=sys.stderr)
        return 2
    OUT.mkdir(parents=True, exist_ok=True)
    caller = Caller(opts.url, token=token, screen="on your screen", keep_audio=True)
    ready = await caller.connect(timeout=30)
    print("session.ready:", {k: ready.get(k) for k in ("engine", "mode", "narrator", "kernel", "tools", "reply")})
    ok = ready.get("mode") == "call" and ready.get("narrator") is True
    await asyncio.sleep(1.5)
    says = opts.say or DEFAULT_SAYS
    try:
        for i, text in enumerate(says):
            pcm = speech.utterance(text, prefer="kokoro")
            print(f"[{caller.now():6.1f}s] you: {text}  ({speech.seconds(pcm):.1f}s of audio)")
            await caller.say(pcm, text)
            before = len(caller.rec.narrations)
            if i == 0:
                # The main agent's reply (highlight), then the sub-agent's report.
                try:
                    await caller.wait_for(lambda: any(n.msg.get("kind") == "highlight" for n in caller.rec.narrations[before:]), opts.timeout, "the highlight")
                    print(f"[{caller.now():6.1f}s] highlight: {caller.rec.narrations[-1].msg.get('text')}")
                except TimeoutError as exc:
                    print("  ", exc)
                try:
                    await caller.wait_for(lambda: any(n.msg.get("kind") == "report" for n in caller.rec.narrations), opts.report_timeout, "the sub-agent report")
                    rep = next(n for n in caller.rec.narrations if n.msg.get("kind") == "report")
                    print(f"[{caller.now():6.1f}s] report: {rep.msg.get('text')}")
                except TimeoutError as exc:
                    print("  ", exc)
                await caller.wait_quiet(2.0, timeout=60)
            else:
                try:
                    await caller.wait_for(lambda: any(n.msg.get("kind") == "detail" for n in caller.rec.narrations[before:]) or len(caller.rec.of("tool.result")) > 0, opts.timeout, "the detail answer")
                    det = next((n for n in caller.rec.narrations[before:] if n.msg.get("kind") == "detail"), None)
                    print(f"[{caller.now():6.1f}s] detail: {det.msg.get('text') if det else caller.rec.of('tool.result')[-1].msg}")
                except TimeoutError as exc:
                    print("  ", exc)
                await caller.wait_quiet(3.0, timeout=60)
        await asyncio.sleep(2.0)
    finally:
        await caller.close()
    rec = caller.rec
    (OUT / "frames.jsonl").write_text("\n".join(json.dumps({"at": round(f.at, 3), **f.msg}) for f in rec.frames))
    lines = ["# Live call transcript", "", f"Gateway: {opts.url.split('?')[0]}", f"session.ready: engine={ready.get('engine')} mode={ready.get('mode')} narrator={ready.get('narrator')} tools={ready.get('tools')}", ""]
    events: list[tuple[float, str]] = []
    for start, end, text in rec.utterances:
        events.append((start, f"you ({end - start:.1f}s): {text}"))
    buf = ""
    buf_at = 0.0
    for f in rec.frames:
        t = f.msg.get("type")
        if t == "transcript.final":
            events.append((f.at, f"heard as: {f.msg.get('text')}"))
        elif t == "narrator.say":
            events.append((f.at, f"narrator [{f.msg.get('kind')}]: {f.msg.get('text')}"))
        elif t == "response.transcript":
            if not buf:
                buf_at = f.at
            buf += str(f.msg.get("text", ""))
        elif t == "response.done":
            if buf.strip():
                events.append((buf_at, f"spoken{' (interrupted)' if f.msg.get('interrupted') else ''}: {buf.strip()}"))
            buf = ""
        elif t == "tool.call":
            events.append((f.at, f"tool call: {f.msg.get('name')} {json.dumps(f.msg.get('arguments'))}"))
        elif t == "tool.result":
            events.append((f.at, f"tool result: {str(f.msg.get('output'))[:300]}"))
        elif t == "error":
            events.append((f.at, f"error: {f.msg.get('message')}"))
    events.sort(key=lambda e: e[0])
    lines += [f"- `{at:6.1f}s` {text}" for at, text in events]
    audio_s = len(rec.pcm) / 2 / speech.RATE
    lines += ["", f"Reply audio: {audio_s:.1f} s ({len(rec.pcm)} bytes PCM16 24 kHz); narrator lines: {len(rec.narrations)}"]
    (OUT / "transcript.md").write_text("\n".join(lines) + "\n")
    with wave.open(str(OUT / "reply.wav"), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(speech.RATE)
        w.writeframes(bytes(rec.pcm))
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", str(OUT / "reply.wav"), "-filter_complex",
                    "showwavespic=s=1400x300:colors=0x4c8bf5", "-frames:v", "1", str(OUT / "reply-waveform.png")], check=False)
    print("\n".join(lines))
    return 0 if ok and rec.narrations else 1


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--url", required=True, help="wss://host/ws (no token in it; VOICE_TOKEN from the env)")
    p.add_argument("--say", action="append", help="an utterance; repeat for more (default: the dispatch + detail script)")
    p.add_argument("--timeout", type=float, default=60.0)
    p.add_argument("--report-timeout", type=float, default=150.0)
    raise SystemExit(asyncio.run(main_async(p.parse_args())))


if __name__ == "__main__":
    main()
