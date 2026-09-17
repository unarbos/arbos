#!/usr/bin/env python3
"""Stub speech server speaking the Arbos voice protocol (SelfHostedVoiceSession.swift).

No real ASR/TTS: every ~0.5 s of received audio yields one scripted word as a
`transcript.delta`; silence for 1.2 s (no frames) ends the utterance with
`speech.stopped` + `transcript.final`. `speak` streams a 440 Hz tone, 1 s per
12 characters, in 100 ms PCM16 24 kHz frames, with `response.transcript` and
`response.done`; `interrupt` stops it at once.

  python voice-stub-server.py [--port 8790] [--token secret] [--words "hello world …"]
"""
import argparse, asyncio, json, math, struct, time
import websockets

RATE = 24000
WORDS = "please list the files in this folder".split()

def tone(seconds, freq=440.0):
    n = int(RATE * seconds)
    return b"".join(struct.pack("<h", int(6000 * math.sin(2 * math.pi * freq * i / RATE))) for i in range(n))

async def handle(ws, token, words):
    if token:
        auth = ws.request.headers.get("Authorization", "")
        if auth != f"Bearer {token}":
            await ws.close(code=4401, reason="bad token"); return
    print("client connected", flush=True)
    state = {"bytes": 0, "words": 0, "speaking": False, "last_audio": None, "speak_task": None, "in_speech": False}

    async def send(obj): await ws.send(json.dumps(obj))

    async def vad_loop():
        while True:
            await asyncio.sleep(0.2)
            la = state["last_audio"]
            if state["in_speech"] and la and time.time() - la > 1.2:
                state["in_speech"] = False
                final = " ".join(words[: state["words"]]) if state["words"] else ""
                await send({"type": "speech.stopped"})
                await send({"type": "transcript.final", "text": final})
                state["bytes"] = 0; state["words"] = 0
                print("final:", final, flush=True)

    async def speak(text):
        state["speaking"] = True
        await send({"type": "response.started"})
        secs = max(0.6, len(text) / 12)
        pcm = tone(secs)
        frame = RATE * 2 // 10  # 100 ms
        sent = 0
        try:
            for i in range(0, len(pcm), frame):
                if not state["speaking"]: break
                await ws.send(pcm[i:i + frame]); sent += 1
                if sent == 3: await send({"type": "response.transcript", "text": text})
                await asyncio.sleep(0.1)
        finally:
            state["speaking"] = False
            await send({"type": "response.done"})
            print(f"speak done ({sent} frames, {'complete' if sent * frame >= len(pcm) else 'interrupted'})", flush=True)

    vad = asyncio.create_task(vad_loop())
    try:
        async for msg in ws:
            if isinstance(msg, bytes):
                state["last_audio"] = time.time()
                if not state["in_speech"]:
                    state["in_speech"] = True
                    await send({"type": "speech.started"})
                    if state["speaking"]:
                        state["speaking"] = False  # barge-in from the server's VAD
                state["bytes"] += len(msg)
                due = state["bytes"] // (RATE * 2 // 2)  # one word per 0.5 s of audio
                while state["words"] < min(due, len(words)):
                    state["words"] += 1
                    await send({"type": "transcript.delta", "text": " ".join(words[: state["words"]])})
                continue
            try: m = json.loads(msg)
            except Exception: continue
            t = m.get("type")
            if t == "session.start":
                await send({"type": "session.ready"})
            elif t == "speak":
                if state["speak_task"] and not state["speak_task"].done():
                    state["speaking"] = False; await state["speak_task"]
                state["speak_task"] = asyncio.create_task(speak(m.get("text", "")))
            elif t == "interrupt":
                print("interrupt", flush=True)
                state["speaking"] = False
            elif t == "session.end":
                break
    finally:
        vad.cancel()
        print("client gone", flush=True)

async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8790)
    ap.add_argument("--token", default="")
    ap.add_argument("--words", default=" ".join(WORDS))
    a = ap.parse_args()
    words = a.words.split()
    async with websockets.serve(lambda ws: handle(ws, a.token, words), "127.0.0.1", a.port, max_size=None):
        print(f"voice stub on ws://127.0.0.1:{a.port}/voice", flush=True)
        await asyncio.Future()

asyncio.run(main())
