"""WebSocket server: auth on the handshake, one Session per connection, /healthz."""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import signal
import sys
from http import HTTPStatus
from urllib.parse import parse_qs, urlsplit

from websockets.asyncio.server import ServerConnection, serve

from . import protocol as P
from .base import SessionDefaults, Tuning
from .duplex import DuplexSession
from .engines import Engines
from .pipeline import PipelineSession

log = logging.getLogger("voice.server")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="voice-server",
        description="Self-hosted full-duplex speech server for Arbos (open-source ASR + TTS over one WebSocket).",
        epilog=P.PROTOCOL_TEXT,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    net = parser.add_argument_group("network")
    net.add_argument("--host", default="127.0.0.1")
    net.add_argument("--port", type=int, default=8765)
    net.add_argument("--token", default=os.environ.get("VOICE_TOKEN", ""),
                     help="shared secret; clients pass ?token= or Authorization: Bearer. Empty = no auth (env VOICE_TOKEN)")

    engine = parser.add_argument_group("engine")
    engine.add_argument("--engine", default="auto", choices=["auto", "duplex", "pipeline"],
                        help="duplex: NemotronLabs VoiceChat via --duplex-url; pipeline: VAD+ASR+TTS; auto: duplex if healthy")
    engine.add_argument("--duplex-url", default=os.environ.get("VOICE_DUPLEX_URL", "ws://127.0.0.1:9000/v1/realtime"),
                        help="NemotronLabs VoiceChat container realtime endpoint")
    engine.add_argument("--instructions", default=None, help="system prompt for the duplex model (text or @file)")

    kernel = parser.add_argument_group("Arbos kernel (enables the agent tools and the text channel)")
    kernel.add_argument("--kernel", default=os.environ.get("VOICE_KERNEL_URL"), help="tcp://127.0.0.1:PORT of `arbos-kernel serve`")
    kernel.add_argument("--kernel-place", default=os.environ.get("VOICE_KERNEL_PLACE"), help="place dir; reads .arbos/kernel.json")
    kernel.add_argument("--auto-approve", action="store_true",
                        help="answer the gateway's OWN kernel's 'allow ...' asks with allow, unasked. Off by default: "
                             "approvals are spoken to the caller and answered by voice or a question card; an "
                             "unanswered one is denied after --approval-timeout. Never applies to a hub attach")
    kernel.add_argument("--no-auto-approve", action="store_true", help=argparse.SUPPRESS)  # old spelling of the default
    kernel.add_argument("--approval-timeout", type=float, default=float(os.environ.get("VOICE_APPROVAL_TIMEOUT", "45")),
                        help="seconds an approval waits for the caller before it is denied with a spoken note")
    kernel.add_argument("--hub", default=os.environ.get("VOICE_HUB_URL"),
                        help="arbos-hub URL (ws[s]://host). A call whose session.start.project names <machine>/<project> "
                             "attaches to that kernel through the hub instead of the gateway's own kernel")
    kernel.add_argument("--hub-token", default=os.environ.get("VOICE_HUB_TOKEN", ""),
                        help="the hub client token (env VOICE_HUB_TOKEN); never logged")
    kernel.add_argument("--hub-machine", default=os.environ.get("VOICE_HUB_MACHINE", ""),
                        help="this gateway's own machine name on the hub, so <machine>/<its place> means its own kernel")

    models = parser.add_argument_group("models")
    models.add_argument("--model-dir", default=os.environ.get("VOICE_MODEL_DIR", "models"),
                        help="holds silero_vad.onnx, kokoro-v1.0.onnx, voices-v1.0.bin (see deploy/run.sh)")
    models.add_argument("--device", default="auto", choices=["auto", "cpu", "cuda"])
    models.add_argument("--asr", default="faster-whisper", choices=["faster-whisper", "none", "mock"],
                        help="none: no recogniser (duplex engine only; the speech model transcribes). "
                             "mock: scripted lines from $VOICE_MOCK_ASR_SCRIPT (test harness)")
    models.add_argument("--asr-model", default=None,
                        help="faster-whisper model name or CTranslate2 dir (default: large-v3-turbo on cuda, small.en on cpu)")
    models.add_argument("--compute-type", default=None, help="CTranslate2 compute type (default: float16 on cuda, int8 on cpu)")
    models.add_argument("--beam-size", type=int, default=3, help="beam for the final transcript; partials use 1")
    models.add_argument("--threads", type=int, default=max(2, min(8, (os.cpu_count() or 4) // 2)), help="CPU threads for ASR")
    models.add_argument("--language", default="en", help="ASR language hint; 'auto' to detect per utterance")
    models.add_argument("--tts", default="kokoro", choices=["kokoro", "tone"],
                        help="tone: a placeholder tone per sentence, no weights (test harness)")
    models.add_argument("--voice", default="af_heart", help="default Kokoro voice (session.start may override)")
    models.add_argument("--speed", type=float, default=1.0)

    reply = parser.add_argument_group("reply hop (answers pipeline voice turns and the text channel)")
    reply.add_argument("--reply", default="none", choices=["none", "openrouter", "kernel"],
                       help="none: speech only, the client sends replies with 'speak'. openrouter: OpenRouter model with the Arbos tools "
                            "(env OPENROUTER_API_KEY). kernel: the kernel's main agent answers")
    reply.add_argument("--reply-model", default="openai/gpt-4.1-mini", help="OpenRouter model id")
    reply.add_argument("--call-model-voice", default=os.environ.get("VOICE_CALL_MODEL_VOICE", "auto"),
                       choices=["auto", "off", "ack", "full"],
                       help="call mode, duplex engine: how much of the speech model's own voice the caller hears. "
                            "auto (default): its answers to small talk and general questions, in its own voice, like "
                            "the phone; work requests go to the main agent and the narrator speaks 'On it.' and the "
                            "results. off: narrator only. ack: short acknowledgements only. full: everything")
    reply.add_argument("--escalations-log",
                       default=os.environ.get("VOICE_ESCALATIONS_LOG")
                       or (os.path.join(os.environ["VOICE_HOME"], "logs", "call-mode-escalations.jsonl") if os.environ.get("VOICE_HOME") else ""),
                       help="call mode: append question-shaped utterances that went to the main agent right after the "
                            "narrator spoke (drill-downs the phrase list may have missed) to this JSONL file. Default: "
                            "$VOICE_HOME/logs/call-mode-escalations.jsonl, or off")
    reply.add_argument("--highlights", default=os.environ.get("VOICE_HIGHLIGHTS", "policy"), choices=["policy", "model"],
                       help="call mode: how the narrator forms a highlight of the agent's reply. policy (default): first "
                            "sentence plus the result sentence, deterministic. model: --narrator-model rewrites the reply "
                            "for the ear, checked against the policy's guardrails (length, no code/links/lists, no numbers "
                            "the reply lacks) and replaced by the policy line on any doubt or after 4 s")
    reply.add_argument("--narrator-model", default=os.environ.get("VOICE_NARRATOR_MODEL") or None,
                       help="call mode: OpenRouter model that turns transcript excerpts into `more_detail` answers "
                            "(needs OPENROUTER_API_KEY). Default: none, the narrator answers from the record verbatim")

    turn = parser.add_argument_group("turn taking (ms)")
    turn.add_argument("--end-silence-ms", type=int, default=600, help="silence that ends an utterance")
    turn.add_argument("--min-speech-ms", type=int, default=96, help="speech before speech.started")
    turn.add_argument("--barge-in-min-ms", type=int, default=256, help="speech before barge-in while the server is talking")
    turn.add_argument("--partial-interval-ms", type=int, default=700, help="how often transcript.delta is attempted")
    turn.add_argument("--vad-threshold", type=float, default=0.5)
    turn.add_argument("--max-lead-ms", type=int, default=1500,
                      help="reply audio is sent at most this far ahead of real-time playback (small = fast interrupt)")
    turn.add_argument("--no-echo-gate", action="store_true", help="do not silence uplink frames that match our own reply audio")
    turn.add_argument("--echo-margin", type=float, default=float(os.environ.get("VOICE_ECHO_MARGIN", "0.7")),
                      help="echo gate: how much louder than the predicted echo the mic must be to pass as the user talking over "
                           "us (0.7). A speakerphone with no echo cancellation (a Mac playing through its speakers) wants 0.9-1.2")

    parser.add_argument("--print-protocol", action="store_true", help="print the wire protocol and exit")
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args(argv)

    if args.device == "auto":
        args.device = "cuda" if _cuda_available() else "cpu"
    if args.asr_model is None:
        args.asr_model = "large-v3-turbo" if args.device == "cuda" else "small.en"
    if args.compute_type is None:
        args.compute_type = "float16" if args.device == "cuda" else "int8"
    if args.language == "auto":
        args.language = None
    if args.instructions and args.instructions.startswith("@"):
        with open(args.instructions[1:], encoding="utf-8") as fh:
            args.instructions = fh.read()
    return args


def _cuda_available() -> bool:
    try:
        import ctranslate2

        return ctranslate2.get_cuda_device_count() > 0
    except Exception:
        return False


def make_process_request(token: str):
    def process_request(connection: ServerConnection, request):
        parts = urlsplit(request.path)
        if parts.path == "/healthz":
            return connection.respond(HTTPStatus.OK, "ok\n")
        if not token:
            return None
        query = parse_qs(parts.query).get("token", [""])[0]
        header = request.headers.get("Authorization", "")
        bearer = header[7:] if header.lower().startswith("bearer ") else ""
        if query == token or bearer == token:
            return None
        log.warning("rejected connection from %s: bad token", connection.remote_address)
        return connection.respond(HTTPStatus.UNAUTHORIZED, "unauthorized\n")

    return process_request


async def serve_forever(args: argparse.Namespace) -> None:
    engines = await Engines.load(args)
    if args.voice not in engines.tts.voices:
        raise SystemExit(f"unknown voice {args.voice!r}; have: {', '.join(engines.tts.voices)}")
    await engines.warm_up(args.voice)
    defaults = SessionDefaults(language=args.language, voice=args.voice, speed=args.speed, reply=args.reply,
                               instructions=args.instructions, narrator_model=args.narrator_model,
                               model_voice=args.call_model_voice, model_highlights=args.highlights == "model",
                               approval_timeout=args.approval_timeout, escalations_log=args.escalations_log or "")
    tuning = Tuning(
        start_threshold=args.vad_threshold,
        end_threshold=max(0.1, args.vad_threshold - 0.15),
        min_speech_ms=args.min_speech_ms,
        barge_in_min_ms=args.barge_in_min_ms,
        end_silence_ms=args.end_silence_ms,
        partial_interval_ms=args.partial_interval_ms,
        max_lead_ms=args.max_lead_ms,
        echo_gate=not args.no_echo_gate,
        echo_margin=args.echo_margin,
    )

    session_class = DuplexSession if engines.engine == "duplex" else PipelineSession

    async def handler(ws: ServerConnection) -> None:
        # Dictation (`session.start {mode: "dictation"}`) is the ASR pipeline whatever the engine:
        # partials as the words come, a final on release, no reply, no speech model. The first
        # frame decides; it is handed to the session so nothing is lost.
        first = await ws.recv()
        klass = session_class
        if isinstance(first, str) and '"dictation"' in first:
            try:
                msg = json.loads(first)
            except json.JSONDecodeError:
                msg = {}
            if msg.get("type") == P.SESSION_START and msg.get("mode") == "dictation":
                klass = PipelineSession
        await klass(ws, engines, defaults, tuning).run(first=first)

    stop = asyncio.get_running_loop().create_future()
    for sig in (signal.SIGINT, signal.SIGTERM):
        asyncio.get_running_loop().add_signal_handler(sig, lambda: stop.done() or stop.set_result(None))

    async with serve(
        handler, args.host, args.port,
        process_request=make_process_request(args.token),
        max_size=4 * 1024 * 1024, ping_interval=20, ping_timeout=20, compression=None,
    ):
        log.info("listening on ws://%s:%d/ws  engine=%s  auth=%s  reply=%s", args.host, args.port,
                 engines.engine, "token" if args.token else "OFF", args.reply)
        await stop


def main(argv: list[str] | None = None) -> None:
    args = parse_args(argv)
    if args.print_protocol:
        print(P.PROTOCOL_TEXT)
        return
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(levelname).1s %(name)s %(message)s",
        datefmt="%H:%M:%S",
        stream=sys.stdout,
    )
    for noisy in ("httpx", "httpcore", "huggingface_hub", "faster_whisper", "websockets"):
        logging.getLogger(noisy).setLevel(logging.WARNING)
    if not args.token:
        log.warning("no --token / VOICE_TOKEN: anyone who can reach the port can use the server")
    asyncio.run(serve_forever(args))
