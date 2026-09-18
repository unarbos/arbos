"""Run the call-mode scenarios against a real gateway with a mock speech model and a mock kernel.

    cd voice-server && source .venv/bin/activate
    python -m tests.run                   # every scenario in tests/scenarios/
    python -m tests.run interrupt-mid-sentence typed-during-call
    python -m tests.run --kokoro          # utterances as Kokoro speech (needs models/); default: synthetic
    python -m tests.run --kernel tcp://127.0.0.1:PORT   # a real kernel instead of the mock (files still checked)
    python -m tests.run --gateway ws://host:port/ws --token T   # an already running gateway (no mocks)

Each scenario is a TOML file: utterances the caller speaks (played as audio into the uplink at real
time), what the mock speech model does with each, what the mock kernel's agents do, and what must
hold at the end: what was spoken, which inbox files were written and with which `channel`, timing.
Exit code 0 when every scenario passed. A JSON report lands in tests/out/report.json; each
scenario's gateway log and place folder stay under tests/out/<name>/ for a look.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import socket
import subprocess
import sys
import time
import tomllib
import traceback
from dataclasses import dataclass, field
from pathlib import Path

import httpx

from tests import speech
from tests.client import Caller
from tests.mock_duplex import MockDuplex, Response, Utterance
from tests.mock_hub import MockHub
from tests.mock_kernel import Behaviour, Child, MockKernel
from tests.mock_openai import MockOpenAILive

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
SCENARIOS = HERE / "scenarios"
OUT = HERE / "out"
TOKEN = "harness"


@dataclass
class Result:
    name: str
    title: str
    ok: bool
    checks: list[tuple[bool, str]] = field(default_factory=list)
    error: str = ""
    seconds: float = 0.0
    spoken: list[str] = field(default_factory=list)
    inbox: list[dict] = field(default_factory=list)


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def load_scenarios(names: list[str]) -> list[dict]:
    files = sorted(SCENARIOS.glob("*.toml"))
    if names:
        files = [f for f in files if f.stem in names]
        missing = set(names) - {f.stem for f in files}
        if missing:
            raise SystemExit(f"no such scenario: {', '.join(sorted(missing))}")
    out = []
    for f in files:
        sc = tomllib.loads(f.read_text())
        sc.setdefault("name", f.stem)
        out.append(sc)
    return out


# ---------------------------------------------------------------------- scripts from steps


def model_response(step: dict) -> Response:
    m = step.get("model") or {}
    if m.get("tool"):
        args = m.get("args") or {"question": step.get("say") or step.get("barge_in") or ""}
        return Response(tool=m["tool"], args=args, after_tool=m.get("after_tool", "{result}"))
    if m.get("ack"):
        return Response(say=m["ack"], words_per_second=float(m.get("words_per_second", 2.5)))
    return Response()


def kernel_behaviour(k: dict) -> Behaviour:
    spawn = None
    if k.get("spawn"):
        s = k["spawn"]
        spawn = Child(name=s["name"], done_after=float(s.get("done_after", 2.0)), last_words=s.get("last_words", "Done."),
                      ok=bool(s.get("ok", True)), tool_output_lines=int(s.get("tool_output_lines", 0)))
    return Behaviour(
        reply=k.get("reply", ""), reply_delay=float(k.get("reply_delay", 0.3)), spawn=spawn,
        reply_after_done=k.get("reply_after_done", ""), ask=k.get("ask", ""), ask_options=list(k.get("ask_options", [])),
        tool_output_lines=int(k.get("tool_output_lines", 0)), steer_reply=k.get("steer_reply", ""),
        tool_seconds=float(k.get("tool_seconds", 1.0)),
        approval=k.get("approval", ""), reply_denied=k.get("reply_denied", ""),
    )


# ---------------------------------------------------------------------- one scenario


class Gateway:
    """The real thing, as a subprocess."""

    def __init__(self, *, duplex_url: str, kernel_url: str, log: Path, extra: list[str], env: dict | None = None):
        self.port = free_port()
        self.url = f"ws://127.0.0.1:{self.port}/ws"
        self.log = log
        self.env = env or {}
        model_dir = OUT / "no-models"
        model_dir.mkdir(parents=True, exist_ok=True)
        self.cmd = [
            sys.executable, "-m", "voice_server", "--host", "127.0.0.1", "--port", str(self.port), "--token", TOKEN,
            "--engine", "duplex", "--duplex-url", duplex_url, "--kernel", kernel_url,
            "--tts", "tone", "--asr", "none", "--reply", "none", "--no-echo-gate", "--max-lead-ms", "300",
            # The scenarios script the speech model's voice and interrupt it: let it through.
            "--call-model-voice", "full",
            "--model-dir", str(model_dir), "-v", *extra,
        ]
        self.proc: subprocess.Popen | None = None

    async def start(self, timeout: float = 30.0) -> None:
        # ARBOS_VOICE_SERVER_SRC: run another checkout's gateway (a branch under review) against
        # this tree's desktop and mocks.
        src = Path(os.environ.get("ARBOS_VOICE_SERVER_SRC") or ROOT)
        env = dict(os.environ, PYTHONPATH=str(src), **self.env)
        self.proc = subprocess.Popen(self.cmd, cwd=src, env=env, stdout=self.log.open("wb"), stderr=subprocess.STDOUT)
        deadline = time.monotonic() + timeout
        async with httpx.AsyncClient(timeout=2.0) as client:
            while time.monotonic() < deadline:
                if self.proc.poll() is not None:
                    raise RuntimeError(f"gateway exited early ({self.proc.returncode}); see {self.log}")
                try:
                    r = await client.get(f"http://127.0.0.1:{self.port}/healthz")
                    if r.status_code == 200:
                        return
                except Exception:
                    pass
                await asyncio.sleep(0.2)
        raise TimeoutError(f"gateway did not come up; see {self.log}")

    def stop(self) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.proc.kill()


class _Refused(Exception):
    """The gateway refused the call, as the scenario expected: nothing more to drive."""


async def run_scenario(sc: dict, opts: argparse.Namespace) -> Result:
    name = sc["name"]
    res = Result(name=name, title=sc.get("title", name), ok=False)
    out = OUT / name
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    place = out / "place"
    place.mkdir()
    started = time.monotonic()

    steps: list[dict] = sc.get("steps", [])
    duplex = MockDuplex()
    kernel = MockKernel(place)
    for step in steps:
        if "say" in step or "barge_in" in step:
            duplex.script.append(Utterance(text=step.get("say") or step.get("barge_in"), response=model_response(step)))
        if "kernel" in step:
            kernel.script.append(kernel_behaviour(step["kernel"]))

    gateway: Gateway | None = None
    caller: Caller | None = None
    live: MockOpenAILive | None = None
    # `hub = { machine, project }`: the scripted kernel sits behind a mock hub under that name;
    # the gateway's own kernel is a second, unscripted one, so routing through the hub is provable.
    hub_cfg = sc.get("hub")
    hub: MockHub | None = None
    own: MockKernel | None = None
    decoy: MockKernel | None = None
    decoy_name = ""
    extra = [*opts.gateway_args, *sc.get("gateway_args", [])]
    project: "str | dict" = ""
    # `path = "own" | "local" | "missing"`: the call names a folder by its path, as the desktop does.
    # own: the gateway's kernel serves that very folder. local: the scripted kernel serves another
    # folder on this host and the gateway's own kernel is a second one — the call must attach to the
    # folder named. missing: a folder with no kernel — the call must be refused, never rerouted.
    path_mode = sc.get("path")
    try:
        duplex_url = await duplex.start()
        kernel_url = opts.kernel or await kernel.start()
        if sc.get("history"):
            transcript = place / ".arbos" / "agents" / "root" / "transcript.jsonl"
            transcript.parent.mkdir(parents=True, exist_ok=True)
            with transcript.open("a", encoding="utf-8") as fh:
                for event in sc["history"]:
                    fh.write(json.dumps(event) + "\n")
        live_env: dict[str, str] = {}
        if sc.get("asr_script"):
            # The gateway transcribes for itself (`--asr mock`, scripted lines), so its own VAD
            # segments the caller's speech — the production shape, which `--asr none` (the
            # speech model's endpointing) does not exercise.
            script = out / "asr-script.txt"
            script.write_text("\n".join(sc["asr_script"]) + "\n")
            live_env["VOICE_MOCK_ASR_SCRIPT"] = str(script)
            extra += ["--asr", "mock"]
        if sc.get("engine") == "openai":
            live = MockOpenAILive()
            live_url = await live.start()
            extra += ["--engine", "openai"]
            live_env = {"OPENAI_API_KEY": "test-key", "VOICE_OPENAI_URL": live_url}
        if path_mode in ("local", "missing"):
            own_place = out / "own-place"
            own_place.mkdir()
            own = MockKernel(own_place)
            own_url = await own.start()
            kernel_url = own_url
            folder = place if path_mode == "local" else (out / "no-kernel-here")
            folder.mkdir(exist_ok=True)
            project = {"machine": "", "project": folder.name, "path": str(folder.resolve()), "name": folder.name,
                       "context": sc.get("context") or {}}
        elif path_mode == "own":
            project = {"machine": "", "project": place.name, "path": str(place.resolve()), "name": place.name,
                       "context": sc.get("context") or {}}
        if hub_cfg:
            own_place = out / "own-place"
            own_place.mkdir()
            own = MockKernel(own_place)
            own_url = await own.start()
            hub = MockHub(token="harness-hub")
            project = f"{hub_cfg['machine']}/{hub_cfg['project']}"
            hub.kernels[project] = kernel_url
            hub.places[project] = str(place)
            # `hub.decoy = "name"`: a second, unscripted kernel on the same machine under another roster
            # name, so a client naming the folder by path must land on the scripted one by `place`.
            if hub_cfg.get("decoy"):
                decoy_place = out / "decoy-place"
                decoy_place.mkdir()
                decoy = MockKernel(decoy_place)
                decoy_url = await decoy.start()
                decoy_name = f"{hub_cfg['machine']}/{hub_cfg['decoy']}"
                hub.kernels[decoy_name] = decoy_url
                hub.places[decoy_name] = str(decoy_place)
            hub_url = await hub.start()
            extra += ["--hub", hub_url, "--hub-token", "harness-hub", "--hub-machine", "gateway-box"]
            kernel_url = own_url
        if opts.gateway:
            url, token = opts.gateway, opts.token
        else:
            gateway = Gateway(duplex_url=duplex_url, kernel_url=kernel_url, log=out / "gateway.log", extra=extra,
                               env=live_env)
            await gateway.start()
            url, token = gateway.url, TOKEN
        # `project = "name"`: a bare folder name, as a desktop off the hub sends it; `refused = "code"`:
        # the gateway must answer with that error code and close, and no kernel may hear a word.
        if sc.get("project"):
            project = sc["project"]
        if isinstance(sc.get("client_project"), dict):
            # The client's own naming of the project (a dict as the desktop sends it); "$PLACE" is the
            # scripted kernel's folder. The hub name the harness checks against stays `project`.
            client_project = {k: (str(place) if v == "$PLACE" else v) for k, v in sc["client_project"].items()}
        else:
            client_project = project
        caller = Caller(url, token=token, screen=sc.get("screen", "on your screen"), project=client_project)
        ready = await caller.connect()
        if sc.get("refused"):
            # `refused = "code"`: the call must be refused with that error code and closed; no kernel hears a word.
            code = ready.get("code") if ready.get("type") == "error" else None
            res.checks.append((code == sc["refused"], f"the gateway refused the call with {sc['refused']} (got {ready.get('type')} {code}: {str(ready.get('message'))[:90]})"))
            await asyncio.sleep(0.6)
            res.checks.append((caller.ready == {}, "no session.ready followed the refusal"))
            res.checks.append((caller.closed is not None and caller.closed[0] == 4404, f"the socket closed with 4404 ({caller.closed})"))
            res.checks.append((len(kernel.users) == 0, f"the gateway's own kernel received no user frames ({len(kernel.users)})"))
            raise _Refused()
        if path_mode == "missing":
            errors = [f.msg for f in caller.rec.frames if f.msg.get("type") == "error"]
            code = errors[-1].get("code") if errors else None
            res.checks.append((not caller.ready, f"no session.ready for a folder with no kernel (got {bool(caller.ready)})"))
            res.checks.append((code == "project_offline", f"the refusal names the cause: error.code = project_offline (got {code})"))
            res.checks.append((caller.closed is not None and caller.closed[0] == 4404, f"the socket closed with 4404 ({caller.closed})"))
            res.checks.append((len(own.users) == 0, f"the gateway's own kernel received no user frames ({len(own.users)})"))
            raise _Refused()
        res.checks.append((ready.get("mode") == "call" and ready.get("narrator") is True, f"session.ready says call mode with a narrator ({ready.get('mode')}, narrator={ready.get('narrator')})"))
        if path_mode in ("own", "local"):
            want = str(place.resolve())
            got = str(ready.get("project_path") or "")
            same = got and os.path.realpath(got) == os.path.realpath(want)
            via = ready.get("via")
            res.checks.append((bool(same), f"session.ready.project_path is the folder the call asked for ({got})"))
            res.checks.append((via == path_mode, f"session.ready.via = {path_mode} (got {via})"))
        if hub_cfg:
            res.checks.append((ready.get("via") == "hub" and ready.get("project") == project, f"session.ready says the call is attached through the hub to {project} (via={ready.get('via')}, project={ready.get('project')})"))
            info = ready.get("project_info") or {}
            res.checks.append((info.get("path") == str(place) and info.get("place") == str(place) and info.get("via") == "hub",
                               f"session.ready.project_info names the project's own folder {place} (got {info.get('path')}, via={info.get('via')}), not the gateway kernel's"))
        await asyncio.sleep(0.4)  # the speech model's session.update lands
        await run_steps(steps, caller, duplex, kernel, opts)
        await caller.wait_quiet(1.0, timeout=10)
        check(sc.get("expect") or {}, res, caller, duplex, kernel, live=live)
        if hub_cfg and own is not None and hub is not None:
            res.checks.append((len(own.users) == 0, f"the gateway's own kernel received no user frames ({len(own.users)})"))
            res.checks.append((project in hub.attaches, f"the hub saw an attach for {project} ({hub.attaches})"))
            if decoy is not None:
                res.checks.append((len(decoy.users) == 0, f"the same-named decoy kernel received no user frames ({len(decoy.users)})"))
                res.checks.append((decoy_name not in hub.attaches, f"the hub saw no attach for the decoy {decoy_name} ({hub.attaches})"))
        if path_mode == "local" and own is not None:
            res.checks.append((len(own.users) == 0, f"the gateway's own kernel (another folder) received no user frames ({len(own.users)})"))
            res.checks.append((len(kernel.users) >= 1, f"the kernel at the named folder received the caller's words ({len(kernel.users)})"))
    except _Refused:
        pass  # the refusal was the expected outcome; its checks are recorded
    except Exception as exc:
        res.error = f"{type(exc).__name__}: {exc}"
        traceback.print_exc()
    finally:
        if caller:
            res.spoken = caller.rec.spoken()
            (out / "frames.jsonl").write_text("\n".join(json.dumps({"at": round(f.at, 3), **f.msg}) for f in caller.rec.frames))
            (out / "audio.jsonl").write_text("\n".join(json.dumps({"at": round(a.at, 3), "size": a.size, "speaking": a.speaking}) for a in caller.rec.audio))
            await caller.close()
        if gateway:
            gateway.stop()
        await duplex.stop()
        await kernel.stop()
        if live:
            await live.stop()
        if hub:
            await hub.stop()
        if own:
            await own.stop()
        if decoy:
            await decoy.stop()
        res.inbox = kernel.inbox_files("root")
        (out / "inbox.json").write_text(json.dumps(res.inbox, indent=1))
    res.seconds = time.monotonic() - started
    res.ok = not res.error and all(ok for ok, _ in res.checks)
    return res


async def run_steps(steps: list[dict], caller: Caller, duplex: MockDuplex, kernel: MockKernel, opts: argparse.Namespace) -> None:
    prefer = "kokoro" if opts.kokoro else "synthetic"
    for step in steps:
        if "say" in step:
            pcm = speech.utterance(step["say"], prefer=prefer)
            text = step["say"]
            if "then" in step:
                # One utterance with a breath in it: `say`, `gap` seconds of silence, `then`.
                gap = bytes(int(float(step.get("gap", 1.0)) * speech.RATE) * 2)
                pcm = pcm + gap + speech.utterance(step["then"], prefer=prefer)
                text = f"{text} {step['then']}"
            await caller.say(pcm, text)
            await asyncio.sleep(float(step.get("pause_after", 0.8)))
        elif "barge_in" in step:
            since = caller.now()
            await caller.wait_for(
                lambda: caller.speaking or any(f.at >= since - 0.5 for f in caller.rec.of("response.started")),
                float(step.get("timeout", 20)), "a reply to interrupt",
            )
            await asyncio.sleep(float(step.get("after", 0.6)))
            step["_barge_at"] = caller.now()
            pcm = speech.utterance(step["barge_in"], prefer=prefer)
            await caller.say(pcm, step["barge_in"])
            await asyncio.sleep(float(step.get("pause_after", 0.8)))
        elif "text" in step:
            await caller.text(step["text"])
        elif "wait_spoken" in step:
            await caller.wait_spoken(step["wait_spoken"], float(step.get("timeout", 25)))
        elif "wait_frame" in step:
            await caller.wait_frame(step["wait_frame"], int(step.get("count", 1)), float(step.get("timeout", 20)))
        elif "wait_tool_result" in step:
            want = step["wait_tool_result"]
            await caller.wait_for(lambda: any(c["name"] == want for c in duplex.tool_calls) and len(duplex.tool_results) >= sum(1 for c in duplex.tool_calls),
                                  float(step.get("timeout", 20)), f"tool result for {want}")
        elif "wait_quiet" in step:
            await caller.wait_quiet(float(step["wait_quiet"]), float(step.get("timeout", 30)))
        elif "pause" in step:
            await asyncio.sleep(float(step["pause"]))
        elif "kernel_restart" in step:
            await kernel.restart(float(step.get("down_for", 1.5)))
        else:
            raise ValueError(f"unknown step: {step}")


# ---------------------------------------------------------------------- expectations


def check(exp: dict, res: Result, caller: Caller, duplex: MockDuplex, kernel: MockKernel,
          live: MockOpenAILive | None = None) -> None:
    rec = caller.rec
    spoken = rec.spoken()
    joined = "\n".join(spoken).lower()
    narrated = rec.narrated()

    def add(ok: bool, what: str) -> None:
        res.checks.append((bool(ok), what))

    for needle in exp.get("spoken_contains", []):
        add(needle.lower() in joined, f"spoken contains {needle!r}")
    for needle in exp.get("spoken_not_contains", []):
        add(needle.lower() not in joined, f"spoken does not contain {needle!r}")
    for needle in exp.get("narrated_contains", []):
        add(any(needle.lower() in n.lower() for n in narrated), f"narrator said {needle!r}")
    if "narration_max_chars" in exp:
        cap = int(exp["narration_max_chars"])
        short = [str(n.msg.get("text", "")) for n in rec.narrations if n.msg.get("kind") != "detail"]
        longest = max((len(n) for n in short), default=0)
        add(longest <= cap, f"every highlight/report/ask/error line is at most {cap} chars (longest {longest})")
    if "detail_max_chars" in exp:
        cap = int(exp["detail_max_chars"])
        details = [str(n.msg.get("text", "")) for n in rec.narrations if n.msg.get("kind") == "detail"]
        longest = max((len(n) for n in details), default=0)
        add(longest <= cap, f"every detail answer is at most {cap} chars (longest {longest})")
    if "narrations_min" in exp:
        add(len(narrated) >= int(exp["narrations_min"]), f"at least {exp['narrations_min']} narrator lines ({len(narrated)})")
    if "narrations_max" in exp:
        add(len(narrated) <= int(exp["narrations_max"]), f"at most {exp['narrations_max']} narrator lines ({len(narrated)})")
    if "inbox" in exp:
        got = [f for f in kernel.inbox_files("root") if f.get("from") == "user"]
        want = exp["inbox"]
        ok = len(got) == len(want) and all(
            all(str(g.get(k, "")) == str(v) for k, v in w.items()) for g, w in zip(got, want)
        )
        add(ok, f"root inbox from the caller = {[(w.get('channel'), w.get('kind')) for w in want]} (got {[(g.get('channel'), g.get('kind'), g.get('body', '')[:30]) for g in got]})")
    if "inbox_bodies" in exp:
        got = [f.get("body", "") for f in kernel.inbox_files("root") if f.get("from") == "user"]
        for body in exp["inbox_bodies"]:
            add(any(body in g for g in got), f"an inbox file holds {body!r}")
    if "done_files" in exp:
        done = [f for f in kernel.inbox_files("root") if f.get("kind") == "done"]
        add(len(done) == int(exp["done_files"]), f"{exp['done_files']} done file(s) reached root ({len(done)})")
    for tool, needle in (exp.get("tool_result_contains") or {}).items():
        outs = [r["output"] for r in duplex.tool_results if any(c["call_id"] == r["call_id"] and c["name"] == tool for c in duplex.tool_calls)]
        add(any(needle.lower() in o.lower() for o in outs), f"{tool} result contains {needle!r} (got {[o[:80] for o in outs]})")
    for tool in exp.get("tools_offered_not", []):
        add(tool not in duplex.tools_offered, f"speech model is not offered {tool} (offered {duplex.tools_offered})")
    for tool in exp.get("tools_offered", []):
        add(tool in duplex.tools_offered, f"speech model is offered {tool}")
    if exp.get("interrupted"):
        add(any(f.msg.get("interrupted") for f in rec.of("response.done")), "a reply ended with response.done {interrupted: true}")
    if "audio_stops_within_ms" in exp:
        limit = float(exp["audio_stops_within_ms"]) / 1000
        ok = True
        detail = "no interrupt sent"
        if rec.interrupts_sent:
            cut = rec.interrupts_sent[0]
            # Audio the client would have played (sent while a reply was open) after the cut.
            late = [a for a in rec.audio if a.speaking and a.at > cut + limit]
            # A new reply after the cut is fine; only audio from the cut reply counts.
            restarted = [f.at for f in rec.of("response.started") if f.at > cut]
            if restarted:
                late = [a for a in late if a.at < restarted[0]]
            ok = not late
            detail = f"{len(late)} reply frames after interrupt+{limit * 1000:.0f}ms"
        add(ok, f"reply audio stops within {limit * 1000:.0f} ms of the interrupt ({detail})")
    if exp.get("narrations_not_during_speech"):
        bad = [
            n.msg.get("text", "")[:40]
            for n in rec.narrations
            if any(start <= n.at <= end for start, end, _ in rec.utterances)
        ]
        add(not bad, f"no narrator line starts while the caller is talking ({bad})")
    if exp.get("narration_after_user_stops"):
        # The child report must begin after the utterance it landed during ended.
        stops = [t for t, k in duplex.events if k == "speech_stopped"]
        reports = [n for n in rec.narrations if n.msg.get("kind") == "report"]
        ok = bool(reports) and bool(stops)
        if ok:
            report_at = caller.t0 + reports[0].at
            ok = report_at >= max(s for s in stops if s <= report_at) if any(s <= report_at for s in stops) else False
        add(ok, "the sub-agent report was spoken after the caller finished talking")
    if "transcript_finals" in exp:
        finals = [f.msg.get("text", "") for f in rec.of("transcript.final")]
        add(len(finals) == int(exp["transcript_finals"]), f"{exp['transcript_finals']} transcript.final (got {finals})")
    if "response_dones" in exp:
        dones = [f for f in rec.of("response.done") if not f.msg.get("interrupted")]
        add(len(dones) == int(exp["response_dones"]), f"{exp['response_dones']} completed reply(ies) (got {len(dones)})")
    for needle in exp.get("kernel_user", []):
        add(any(needle.lower() in str(u.get("text", "")).lower() for u in kernel.users), f"the main agent was asked {needle!r} ({[str(u.get('text', ''))[:60] for u in kernel.users]})")
    if "mock_seen" in exp:
        add(duplex.seen == list(exp["mock_seen"]), f"speech model heard {exp['mock_seen']} (got {duplex.seen})")
    if "barge_ins" in exp:
        add(duplex.barge_ins >= int(exp["barge_ins"]), f"speech model saw {exp['barge_ins']} barge-in(s) ({duplex.barge_ins})")
    if "text_forwarded" in exp:
        done = [f for f in rec.of("text.done") if f.msg.get("forwarded")]
        add(len(done) >= int(exp["text_forwarded"]), f"{exp['text_forwarded']} typed line(s) forwarded to the agent ({len(done)})")
    if "approvals" in exp:
        got = [bool(a.get("allow")) for a in kernel.approvals]
        add(got == list(exp["approvals"]), f"kernel received approve frames {exp['approvals']} (got {got}; ids {[a.get('call_id') for a in kernel.approvals]})")
    if "approval_answered_within_s" in exp:
        asks = [f.at for f in rec.frames if f.msg.get("type") == "narrator.say" and f.msg.get("kind") == "approval"]
        limit = float(exp["approval_answered_within_s"])
        ok = len(asks) >= 2 and asks[1] - asks[0] <= limit
        add(ok, f"the approval was closed within {limit:.0f} s of being spoken ({[round(a, 1) for a in asks]})")
    if "activity_states" in exp:
        got = [(f.msg.get("agent"), f.msg.get("state")) for f in rec.of("agent.activity") if not f.msg.get("heartbeat")]
        root = [st for ag, st in got if ag == "root"]
        add(root == list(exp["activity_states"]), f"root's agent.activity transitions are {exp['activity_states']} (got {root})")
    if "activity_tool" in exp:
        tools = [f.msg.get("tool") for f in rec.of("agent.activity") if f.msg.get("state") == "tool"]
        add(exp["activity_tool"] in tools, f"agent.activity named the tool {exp['activity_tool']!r} (got {tools})")
    if "activity_heartbeats_min" in exp:
        beats = [f for f in rec.of("agent.activity") if f.msg.get("heartbeat")]
        add(len(beats) >= int(exp["activity_heartbeats_min"]), f"at least {exp['activity_heartbeats_min']} heartbeat(s) while work ran ({len(beats)})")
    if exp.get("activity_idle_last"):
        acts = rec.of("agent.activity")
        add(bool(acts) and acts[-1].msg.get("state") == "idle", "the last agent.activity frame says idle")
    if "user_frames" in exp:
        add(len(kernel.users) == int(exp["user_frames"]), f"kernel received {exp['user_frames']} user frame(s) ({len(kernel.users)})")
    for needle in exp.get("kernel_user_not", []):
        add(not any(needle.lower() in str(u.get("text", "")).lower() for u in kernel.users), f"the main agent was not asked {needle!r}")
    if "response_started_min" in exp:
        add(len(rec.of("response.started")) >= int(exp["response_started_min"]), f"at least {exp['response_started_min']} replies started ({len(rec.of('response.started'))})")
    if "audio_bytes_min" in exp:
        total = sum(a.size for a in rec.audio if a.speaking)
        add(total >= int(exp["audio_bytes_min"]), f"at least {exp['audio_bytes_min']} bytes of reply audio ({total})")
    if live is not None and (exp.get("live_sees") or exp.get("live_not_sees") or exp.get("live_started")):
        blob = (live.instructions + "\n" + live.input_text()).lower()
        add(live.started >= 1, f"GPT-Live received session.start ({live.started})")
        for needle in exp.get("live_sees", []):
            add(needle.lower() in blob, f"GPT-Live session sees {needle!r}")
        for needle in exp.get("live_not_sees", []):
            add(needle.lower() not in blob, f"GPT-Live session does not see {needle!r}")


# ---------------------------------------------------------------------- main


def table(results: list[Result]) -> str:
    width = max(len(r.name) for r in results) if results else 8
    lines = [f"{'scenario'.ljust(width)}  result  time   detail", f"{'-' * width}  ------  -----  ------"]
    for r in results:
        failed = [w for ok, w in r.checks if not ok]
        detail = r.error or ("; ".join(failed) if failed else f"{sum(1 for ok, _ in r.checks if ok)} checks")
        lines.append(f"{r.name.ljust(width)}  {'PASS' if r.ok else 'FAIL':6}  {r.seconds:5.1f}  {detail}")
    return "\n".join(lines)


async def main_async(opts: argparse.Namespace) -> int:
    OUT.mkdir(exist_ok=True)
    scenarios = load_scenarios(opts.names)
    if opts.kokoro and not speech.kokoro_available():
        print(f"--kokoro asked but no weights in {speech.MODELS}; run deploy/fetch-models.sh models", file=sys.stderr)
        return 2
    results: list[Result] = []
    for sc in scenarios:
        print(f"== {sc['name']}: {sc.get('title', '')}", flush=True)
        r = await run_scenario(sc, opts)
        for ok, what in r.checks:
            print(f"   {'ok  ' if ok else 'FAIL'} {what}")
        if r.error:
            print(f"   ERROR {r.error}")
        for s in r.spoken:
            print(f"   spoken: {s[:140]}")
        results.append(r)
    print()
    print(table(results))
    (OUT / "report.json").write_text(json.dumps([
        {"name": r.name, "title": r.title, "ok": r.ok, "seconds": round(r.seconds, 1), "error": r.error,
         "checks": [{"ok": ok, "what": w} for ok, w in r.checks], "spoken": r.spoken,
         "inbox": [{k: v for k, v in f.items() if k != "body"} | {"body": f.get("body", "")[:120]} for f in r.inbox]}
        for r in results
    ], indent=1))
    return 0 if all(r.ok for r in results) else 1


def main() -> None:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("names", nargs="*", help="scenario names (default: all)")
    p.add_argument("--kokoro", action="store_true", help="utterances as Kokoro speech instead of the synthetic signal")
    p.add_argument("--kernel", default="", help="tcp://host:port of a real kernel instead of the mock")
    p.add_argument("--gateway", default="", help="ws URL of a running gateway instead of starting one (mocks are still started but unused)")
    p.add_argument("--token", default=TOKEN)
    p.add_argument("--gateway-args", nargs=argparse.REMAINDER, default=[], help="extra args for the gateway process")
    opts = p.parse_args()
    raise SystemExit(asyncio.run(main_async(opts)))


if __name__ == "__main__":
    main()
