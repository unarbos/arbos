"""A stand-in for the NemotronLabs VoiceChat container: the realtime protocol `duplex.py` speaks,
with scripted ears and a scripted mouth, no GPU.

Ears: an energy VAD on the uplink audio decides when the caller starts and stops talking, so the
harness's WAV utterances and pauses drive `speech_started` / `speech_stopped` for real. The N-th
utterance is "recognised" as the N-th scripted line (the mock does not do ASR).

Mouth: for each utterance the script says what the model does: speak a sentence (a tone with a
transcript, paced at real time), call a tool, or stay quiet. A caller who starts talking while the
mock is speaking barges in: the audio stops and `response.done` follows, as the real model does.

Also serves `GET /v1/realtime/health` and `GET /` so `engines.probe_duplex` picks it as healthy.
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import time
import uuid
from dataclasses import dataclass, field
from http import HTTPStatus

import numpy as np
from websockets.asyncio.server import ServerConnection, serve

log = logging.getLogger("mock.duplex")

RATE = 24_000
CHUNK_MS = 80
CHUNK = RATE * CHUNK_MS // 1000
SPEECH_RMS = 0.012  # uplink loudness that counts as talking
START_MS = 200  # this much talking opens an utterance
STOP_MS = 600  # this much quiet closes it


@dataclass
class Response:
    """What the mock says or does after one utterance."""

    say: str = ""  # spoken (tone + transcript). "" = silence
    tool: str = ""  # call this tool with `args` instead of / before speaking
    args: dict = field(default_factory=dict)
    after_tool: str = ""  # spoken after the tool result arrives; "{result}" is replaced by it
    words_per_second: float = 2.5  # how long the tone for `say` lasts


@dataclass
class Utterance:
    text: str
    response: Response


class MockDuplex:
    def __init__(self, host: str = "127.0.0.1", port: int = 0):
        self.host, self.port = host, port
        self.script: list[Utterance] = []
        self.seen: list[str] = []  # utterances "recognised", in order
        self.tool_calls: list[dict] = []  # {name, arguments, call_id}
        self.tool_results: list[dict] = []  # {call_id, output}
        self.instructions = ""
        self.tools_offered: list[str] = []
        self.spoken: list[str] = []  # transcripts the mock voiced (complete or cut)
        self.barge_ins = 0
        self.events: list[tuple[float, str]] = []  # (monotonic, kind) for timing assertions
        self._server = None
        self._ws: ServerConnection | None = None
        self._speaking: asyncio.Task | None = None
        self._tool_waiters: dict[str, asyncio.Future] = {}
        self._utterances = 0

    # ------------------------------------------------------------------ lifecycle

    async def start(self) -> str:
        self._server = await serve(self._handler, self.host, self.port, process_request=self._http,
                                   max_size=16 * 1024 * 1024, compression=None)
        self.port = self._server.sockets[0].getsockname()[1]
        return f"ws://{self.host}:{self.port}/v1/realtime"

    async def stop(self) -> None:
        if self._server:
            self._server.close()
            await self._server.wait_closed()

    def _http(self, connection: ServerConnection, request):
        if request.path.startswith("/v1/realtime/health"):
            return connection.respond(HTTPStatus.OK, json.dumps({"status": "ok"}))
        if request.path == "/":
            return connection.respond(HTTPStatus.OK, json.dumps({"model_name": "mock-voicechat"}))
        return None

    # ------------------------------------------------------------------ one gateway connection

    async def _handler(self, ws: ServerConnection) -> None:
        self._ws = ws
        await self._send({"type": "session.created", "session": {"id": "mock"}})
        talking = False
        run_ms = 0
        quiet_ms = 0
        try:
            async for raw in ws:
                try:
                    msg = json.loads(raw)
                except json.JSONDecodeError:
                    continue
                kind = msg.get("type")
                if kind == "session.update":
                    session = msg.get("session") or {}
                    self.instructions = session.get("instructions", "")
                    self.tools_offered = [t.get("name", "") for t in session.get("tools") or []]
                    await self._send({"type": "session.updated"})
                elif kind == "input_audio_buffer.append":
                    pcm = base64.b64decode(msg.get("audio", ""))
                    if not pcm:
                        continue
                    x = np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768.0
                    ms = int(1000 * x.size / RATE)
                    loud = float(np.sqrt(np.mean(x * x))) > SPEECH_RMS
                    if not talking:
                        run_ms = run_ms + ms if loud else 0
                        if run_ms >= START_MS:
                            talking, quiet_ms = True, 0
                            await self._speech_started()
                    else:
                        quiet_ms = 0 if loud else quiet_ms + ms
                        if quiet_ms >= STOP_MS:
                            talking, run_ms = False, 0
                            await self._speech_stopped()
                elif kind == "conversation.item.create":
                    item = msg.get("item") or {}
                    if item.get("type") == "function_call_output":
                        self.tool_results.append({"call_id": item.get("call_id"), "output": item.get("output", "")})
                        fut = self._tool_waiters.pop(item.get("call_id", ""), None)
                        if fut and not fut.done():
                            fut.set_result(str(item.get("output", "")))
                elif kind == "response.cancel":
                    await self._stop_speaking(reason="cancel")
                elif kind == "session.close":
                    break
        finally:
            await self._stop_speaking(reason="close")
            self._ws = None

    async def _send(self, msg: dict) -> None:
        if self._ws is None:
            return
        msg.setdefault("event_id", str(uuid.uuid4()))
        try:
            await self._ws.send(json.dumps(msg))
        except Exception:
            pass

    # ------------------------------------------------------------------ ears

    async def _speech_started(self) -> None:
        self.events.append((time.monotonic(), "speech_started"))
        # The real model reports the caller first and yields a beat later.
        await self._send({"type": "input_audio_buffer.speech_started"})
        if self._speaking and not self._speaking.done():
            self.barge_ins += 1
            await asyncio.sleep(0.15)
            await self._stop_speaking(reason="barge-in")

    async def _speech_stopped(self) -> None:
        self.events.append((time.monotonic(), "speech_stopped"))
        await self._send({"type": "input_audio_buffer.speech_stopped"})
        n = self._utterances
        self._utterances += 1
        if n < len(self.script):
            utt = self.script[n]
            text = utt.text
        else:
            utt, text = None, f"(unscripted utterance {n + 1})"
        self.seen.append(text)
        await self._send({"type": "conversation.item.input_audio_transcription.completed", "transcript": text})
        if utt is not None:
            self._speaking = asyncio.create_task(self._respond(utt.response))

    # ------------------------------------------------------------------ mouth

    async def _respond(self, r: Response) -> None:
        await self._send({"type": "response.created"})
        try:
            if r.tool:
                call_id = f"call_{uuid.uuid4().hex[:8]}"
                fut: asyncio.Future = asyncio.get_running_loop().create_future()
                self._tool_waiters[call_id] = fut
                self.tool_calls.append({"name": r.tool, "arguments": r.args, "call_id": call_id})
                await self._send({
                    "type": "response.function_call_arguments.done",
                    "name": r.tool, "call_id": call_id, "arguments": json.dumps(r.args),
                })
                try:
                    result = await asyncio.wait_for(fut, 30)
                except asyncio.TimeoutError:
                    result = "(no tool result)"
                if r.after_tool:
                    await self._speak(r.after_tool.replace("{result}", result), r.words_per_second)
            elif r.say:
                await self._speak(r.say, r.words_per_second)
        except asyncio.CancelledError:
            raise
        finally:
            await self._send({"type": "response.done"})

    async def _speak(self, text: str, wps: float) -> None:
        """A 180 Hz tone as long as the words would take, sent in real time with the transcript
        leading it, as the real model does."""
        words = text.split()
        seconds = max(0.4, len(words) / max(0.5, wps))
        frames = int(seconds * 1000 / CHUNK_MS)
        per_frame = max(1, len(words) // max(1, frames // 2))
        spoken: list[str] = []
        self.events.append((time.monotonic(), "response_audio_start"))
        try:
            for i in range(frames):
                if i * per_frame < len(words) and i % 2 == 0:
                    piece = " ".join(words[(i // 2) * per_frame : (i // 2 + 1) * per_frame])
                    if piece:
                        spoken.append(piece)
                        await self._send({"type": "response.output_audio_transcript.delta", "delta": piece + " "})
                t = (np.arange(CHUNK) + i * CHUNK) / RATE
                pcm = (0.25 * np.sin(2 * np.pi * 180.0 * t) * 32767).astype("<i2").tobytes()
                await self._send({"type": "response.output_audio.delta", "delta": base64.b64encode(pcm).decode()})
                await asyncio.sleep(CHUNK_MS / 1000)
            leftover = " ".join(words[len(spoken) * per_frame :]) if spoken else ""
            if leftover and not any(leftover in s for s in spoken):
                await self._send({"type": "response.output_audio_transcript.delta", "delta": leftover})
            await self._send({"type": "response.output_audio_transcript.done", "transcript": text})
        finally:
            self.spoken.append(" ".join(spoken) if spoken else text)
            self.events.append((time.monotonic(), "response_audio_end"))

    async def _stop_speaking(self, *, reason: str) -> None:
        task = self._speaking
        if task and not task.done():
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, Exception):
                pass
            self.events.append((time.monotonic(), f"stopped:{reason}"))
        self._speaking = None
