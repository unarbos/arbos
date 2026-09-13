#!/usr/bin/env python3
"""Test client: streams a WAV like a microphone, prints the live transcript, saves or
plays the reply audio, measures latency, and (optionally) barges in over the reply.

    python client.py ws://127.0.0.1:8765/ws?token=SECRET --wav q.wav --speak "Hi there."
    python client.py wss://voice-api.arbos.life/ws?token=SECRET --wav q.wav --reply --barge-in q2.wav
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
import time
import wave
from pathlib import Path

import numpy as np
import websockets

from voice_server.audio import float_to_pcm16, pcm16_to_float, resample_whole

RATE = 24_000
FRAME_MS = 40
FRAME = RATE * FRAME_MS // 1000
LONG_TEXT = (
    "Here is a longer answer so you have time to interrupt me. The first thing to know is that the "
    "kernel runs one folder per agent, and each agent keeps its state on disk. The second thing is "
    "that sub-agents are spawned as children with their own folders, so you can always read what "
    "they are doing. The third thing is that the whole tree can be rewound from git."
)


def load_wav(path: str) -> np.ndarray:
    with wave.open(path, "rb") as w:
        rate, channels, width = w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(w.getnframes())
    if width != 2:
        raise SystemExit(f"{path}: need 16-bit PCM, got {width * 8}-bit")
    x = pcm16_to_float(raw)
    if channels > 1:
        x = x.reshape(-1, channels).mean(axis=1)
    return resample_whole(x, rate, RATE)


def last_speech_sample(x: np.ndarray, thresh: float = 0.01) -> int:
    win = RATE // 50
    rms = np.sqrt(np.convolve(x * x, np.ones(win) / win, mode="same"))
    loud = np.flatnonzero(rms > thresh)
    return int(loud[-1]) if loud.size else x.size


def first_speech_sample(x: np.ndarray, thresh: float = 0.01) -> int:
    win = RATE // 50
    rms = np.sqrt(np.convolve(x * x, np.ones(win) / win, mode="same"))
    loud = np.flatnonzero(rms > thresh)
    return int(loud[0]) if loud.size else 0


class Mic:
    """Sends 40 ms frames in real time, forever: WAV audio when queued, silence otherwise."""

    def __init__(self, ws):
        self.ws = ws
        self.queue: list[tuple[np.ndarray, dict]] = []
        self.marks: dict[str, float] = {}

    def play(self, x: np.ndarray, name: str) -> None:
        self.queue.append((x, {"name": name, "first": first_speech_sample(x), "last": last_speech_sample(x)}))

    async def run(self) -> None:
        pos, cur = 0, None
        next_at = time.monotonic()
        while True:
            if cur is None and self.queue:
                cur, pos = self.queue.pop(0), 0
            if cur is not None:
                x, meta = cur
                frame = x[pos : pos + FRAME]
                if pos <= meta["first"] < pos + FRAME:
                    self.marks[meta["name"] + ".speech_start"] = time.monotonic()
                if pos <= meta["last"] < pos + FRAME or (pos + FRAME >= x.size and meta["name"] + ".speech_end" not in self.marks):
                    self.marks[meta["name"] + ".speech_end"] = time.monotonic()
                pos += FRAME
                if pos >= x.size:
                    cur = None
                if frame.size < FRAME:
                    frame = np.concatenate([frame, np.zeros(FRAME - frame.size, dtype=np.float32)])
            else:
                frame = np.zeros(FRAME, dtype=np.float32)
            await self.ws.send(float_to_pcm16(frame))
            next_at += FRAME_MS / 1000
            await asyncio.sleep(max(0.0, next_at - time.monotonic()))


class Player:
    def __init__(self, enabled: bool):
        self.stream = None
        if enabled:
            import sounddevice as sd

            self.stream = sd.RawOutputStream(samplerate=RATE, channels=1, dtype="int16")
            self.stream.start()

    def write(self, pcm: bytes) -> None:
        if self.stream:
            self.stream.write(pcm)

    def flush(self) -> None:
        if self.stream:
            self.stream.stop()
            self.stream.start()


class Run:
    def __init__(self, args):
        self.args = args
        self.t0 = time.monotonic()
        self.events: list[tuple[float, str, dict]] = []
        self.audio = bytearray()
        self.audio_frames = 0
        self.first_audio_at: float | None = None
        self.late_audio_frames = 0
        self.interrupted_at: float | None = None
        self.waiters: dict[str, asyncio.Future] = {}
        self.line = ""
        self.text_line = ""
        self.ws = None
        self.response_open = False
        self.player = Player(args.play)
        self.metrics: dict[str, float] = {}

    def log(self, text: str) -> None:
        print(f"[{time.monotonic() - self.t0:7.3f}] {text}", flush=True)

    def wait(self, kind: str) -> asyncio.Future:
        fut = asyncio.get_running_loop().create_future()
        self.waiters[kind] = fut
        return fut

    def on_message(self, message) -> None:
        now = time.monotonic()
        if isinstance(message, (bytes, bytearray)):
            self.audio_frames += 1
            if self.interrupted_at is not None and now - self.interrupted_at < 1.0:
                self.late_audio_frames += 1  # reply audio that kept coming right after the interrupt
            if self.first_audio_at is None:
                self.first_audio_at = now
                self.log(f"<- audio: first frame ({len(message)} bytes)")
                self._resolve("audio.first", {})
            self.audio += message
            self.player.write(bytes(message))
            return
        msg = json.loads(message)
        kind = msg.pop("type")
        self.events.append((now, kind, msg))
        if kind == "transcript.delta":
            self.line += msg.get("text", "")
            self.log(f"<- transcript.delta: you: {self.line}")
            return
        if kind == "transcript.final":
            self.log(f"<- transcript.final: you: {msg.get('text')}")
            self.line = ""
        elif kind == "response.done" and msg.get("interrupted"):
            self.interrupted_at = now
            self.player.flush()
            self.log(f"<- {kind} {msg}")
        elif kind == "speech.started":
            self.player.flush()
            self.log(f"<- {kind}")
            if self.response_open and self.ws is not None:  # what the iOS client does: stop playback, tell the server
                self.response_open = False
                asyncio.create_task(self.ws.send(json.dumps({"type": "interrupt"})))
                self.log("-> interrupt (barge-in)")
        elif kind == "response.started":
            self.response_open = True
            self.log(f"<- {kind}")
        elif kind == "response.done":
            self.response_open = False
            self.log(f"<- {kind} {msg if msg else ''}")
        elif kind == "text.delta":
            self.text_line += msg.get("text", "")
            if self.waiters.get("text.delta"):
                self.log(f"<- text.delta (streaming)")
        elif kind == "agent.event":
            text = (msg.get("text") or "").replace("\n", " ")
            if msg.get("kind") != "assistant":
                self.log(f"<- agent.event {msg.get('agent')} {msg.get('kind')}: {text[:100]}")
        elif kind in ("agent.turn", "agent.tree"):
            pass
        else:
            self.log(f"<- {kind} {msg if msg else ''}")
        self._resolve(kind, msg)

    def _resolve(self, kind: str, msg: dict) -> None:
        fut = self.waiters.pop(kind, None)
        if fut is not None and not fut.done():
            fut.set_result((time.monotonic(), msg))


async def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("url", help="ws://host:port/ws?token=... or wss://...")
    ap.add_argument("--wav", required=True, help="16-bit WAV with one utterance; streamed as the microphone")
    ap.add_argument("--speak", default=None, help="text to have the server voice after the transcript (speech-only mode)")
    ap.add_argument("--reply", action="store_true", help="expect the server to answer on its own (server ran with --reply)")
    ap.add_argument("--barge-in", metavar="WAV", default=None, help="second utterance to stream over the reply")
    ap.add_argument("--play", action="store_true", help="play the reply through the speakers (needs sounddevice)")
    ap.add_argument("--text", default=None, help="also send this on the text channel (text.input) and time the streamed answer")
    ap.add_argument("--agent-wav", default=None,
                    help="third utterance that asks for work ('send an agent to ...'); waits for tool.call and agent.done")
    ap.add_argument("--out", default="out", help="directory for reply WAVs and metrics.json")
    ap.add_argument("--timeout", type=float, default=60.0)
    ap.add_argument("--agent-timeout", type=float, default=240.0, help="how long to wait for agent.done")
    args = ap.parse_args()

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    run = Run(args)
    question = load_wav(args.wav)

    async with websockets.connect(args.url, max_size=4 * 1024 * 1024, compression=None) as ws:
        run.log(f"connected to {args.url.split('?')[0]}")
        run.ws = ws
        mic = Mic(ws)
        reader = asyncio.create_task(_read(ws, run))
        ready = run.wait("session.ready")
        t_start = time.monotonic()
        await ws.send(json.dumps({"type": "session.start", "format": {"type": "audio/pcm", "rate": RATE}}))
        run.log("-> session.start")
        _at, ready_msg = await asyncio.wait_for(ready, args.timeout)
        run.metrics["session.ready_ms"] = (_at - t_start) * 1000
        mic_task = asyncio.create_task(mic.run())

        # 1. the question
        final = run.wait("transcript.final")
        stopped = run.wait("speech.stopped")
        run.log(f"-> streaming {args.wav} ({question.size / RATE:.1f}s) as the microphone")
        mic.play(question, "q1")
        t_stop, _ = await asyncio.wait_for(stopped, args.timeout)
        t_final, final_msg = await asyncio.wait_for(final, args.timeout)
        speech_end = mic.marks["q1.speech_end"]
        run.metrics["asr.speech_stopped_ms"] = (t_stop - speech_end) * 1000
        run.metrics["asr.final_ms"] = (t_final - speech_end) * 1000
        run.metrics["asr.text"] = final_msg.get("text", "")

        # 2. the reply: either the server's own, or text we hand it
        if args.reply:
            started = run.wait("response.started")
            first_tr = run.wait("response.transcript")
            first_audio = run.wait("audio.first")
            done = run.wait("response.done")
            await asyncio.wait_for(started, args.timeout)
            t_tr, _ = await asyncio.wait_for(first_tr, args.timeout)
            run.metrics["reply.first_token_ms"] = (t_tr - t_final) * 1000
            t_audio, _ = await asyncio.wait_for(first_audio, args.timeout)
            run.metrics["reply.first_audio_ms"] = (t_audio - t_final) * 1000
            run.metrics["e2e.speech_end_to_first_audio_ms"] = (t_audio - speech_end) * 1000
        else:
            default_text = LONG_TEXT if args.barge_in else "I heard you. This is the voice server talking back."
            text = args.speak or default_text
            first_audio = run.wait("audio.first")
            done = run.wait("response.done")
            t_speak = time.monotonic()
            await ws.send(json.dumps({"type": "speak", "text": text}))
            run.log(f"-> speak ({len(text)} chars)")
            t_audio, _ = await asyncio.wait_for(first_audio, args.timeout)
            run.metrics["tts.first_audio_ms"] = (t_audio - t_speak) * 1000
            run.metrics["e2e.speech_end_to_first_audio_ms"] = (t_audio - speech_end) * 1000

        # 3. barge-in: talk over the reply
        if args.barge_in:
            await asyncio.sleep(0.6)
            second = load_wav(args.barge_in)
            started = run.wait("speech.started")
            final2 = run.wait("transcript.final")
            done = run.wait("response.done")  # the first end-of-reply after we start talking over it
            run.log(f"-> barge-in: streaming {args.barge_in} over the reply")
            mic.play(second, "q2")
            t_started, _ = await asyncio.wait_for(started, args.timeout)
            t_done, done_msg = await asyncio.wait_for(done, args.timeout)
            start2 = mic.marks["q2.speech_start"]
            run.metrics["bargein.speech_started_ms"] = (t_started - start2) * 1000
            run.metrics["bargein.response_done_ms"] = (t_done - start2) * 1000
            run.metrics["bargein.interrupted"] = bool(done_msg.get("interrupted"))
            t_final2, final2_msg = await asyncio.wait_for(final2, args.timeout)
            run.metrics["bargein.second_final_ms"] = (t_final2 - mic.marks["q2.speech_end"]) * 1000
            run.metrics["bargein.second_text"] = final2_msg.get("text", "")
            await asyncio.sleep(0.5)
            run.metrics["bargein.late_audio_frames"] = run.late_audio_frames
        else:
            t_done, _ = await asyncio.wait_for(done, args.timeout)
            run.metrics["reply.done_ms"] = (t_done - (t_final if args.reply else t_speak)) * 1000

        # 4. text channel
        if args.text:
            first = run.wait("text.delta")
            done = run.wait("text.done")
            t_text = time.monotonic()
            await ws.send(json.dumps({"type": "text.input", "text": args.text}))
            run.log(f"-> text.input: {args.text!r}")
            t_first, _ = await asyncio.wait_for(first, args.timeout)
            t_done, done_msg = await asyncio.wait_for(done, args.agent_timeout)
            run.metrics["text.first_token_ms"] = (t_first - t_text) * 1000
            run.metrics["text.done_ms"] = (t_done - t_text) * 1000
            run.metrics["text.answer"] = done_msg.get("text", "")[:200]
            run.log(f"<- text answer: {done_msg.get('text', '')[:200]!r}")

        # 5. acting: ask for work by voice, expect a tool call and, later, the agent's report
        if args.agent_wav:
            await asyncio.sleep(1.0)
            third = load_wav(args.agent_wav)
            final3 = run.wait("transcript.final")
            call = run.wait("tool.call")
            result = run.wait("tool.result")
            agent_done = run.wait("agent.done")
            run.first_audio_at = None
            first_audio = run.wait("audio.first")
            run.log(f"-> streaming {args.agent_wav} (asks for an agent)")
            mic.play(third, "q3")
            t_final3, final3_msg = await asyncio.wait_for(final3, args.timeout)
            run.metrics["act.transcript"] = final3_msg.get("text", "")
            t_call, call_msg = await asyncio.wait_for(call, args.timeout)
            run.metrics["act.tool_call_ms"] = (t_call - mic.marks["q3.speech_end"]) * 1000
            run.metrics["act.tool"] = f"{call_msg.get('name')}({json.dumps(call_msg.get('arguments'))[:120]})"
            t_result, result_msg = await asyncio.wait_for(result, args.timeout)
            run.metrics["act.tool_result_ms"] = (t_result - t_call) * 1000
            run.metrics["act.tool_output"] = str(result_msg.get("output", ""))[:200]
            t_audio3, _ = await asyncio.wait_for(first_audio, args.timeout)
            run.metrics["act.first_audio_ms"] = (t_audio3 - mic.marks["q3.speech_end"]) * 1000
            t_agent, agent_msg = await asyncio.wait_for(agent_done, args.agent_timeout)
            run.metrics["act.agent_done_s"] = (t_agent - t_call)
            run.metrics["act.agent"] = agent_msg.get("agent", "")
            run.metrics["act.report"] = str(agent_msg.get("text", ""))[:200]
            run.log("waiting for the spoken report to finish")
            await asyncio.sleep(6.0)

        run.metrics["reply.audio_seconds"] = len(run.audio) / 2 / RATE
        await ws.send(json.dumps({"type": "session.end"}))
        mic_task.cancel()
        reader.cancel()

    wav_path = out_dir / "reply.wav"
    with wave.open(str(wav_path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(bytes(run.audio))
    run.metrics["server"] = {k: ready_msg.get(k) for k in ("asr", "tts", "reply", "voice")}
    (out_dir / "metrics.json").write_text(json.dumps(run.metrics, indent=2))
    print()
    print(f"reply audio: {wav_path} ({run.metrics['reply.audio_seconds']:.1f}s, {run.audio_frames} frames)")
    print()
    print("| metric | value |")
    print("|---|---|")
    for key, value in run.metrics.items():
        if isinstance(value, float):
            print(f"| {key} | {value:.0f} |" if key.endswith("_ms") else f"| {key} | {value:.2f} |")
        elif not isinstance(value, dict):
            print(f"| {key} | {value} |")


async def _read(ws, run: Run) -> None:
    try:
        async for message in ws:
            run.on_message(message)
    except websockets.ConnectionClosed as exc:
        run.log(f"connection closed: {exc}")


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        sys.exit(130)
