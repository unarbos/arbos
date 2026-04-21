"""Drive a single ``cursor-agent`` invocation and stream its output.

We spawn ``cursor-agent -p --output-format stream-json --stream-partial-output
--force`` as a subprocess, parse its newline-delimited JSON stream, and call
back into the Telegram layer with two things:

* ``on_update(text)`` -- a fresh, full snapshot of "what the bubble should
  show right now" (thinking buffer + answer-so-far). Called frequently; the
  consumer is responsible for throttling.
* ``on_final(text)`` -- the final result text once the agent finishes (or an
  error message if it failed).

This module deliberately knows nothing about Telegram; it's a thin shell
around the cursor-agent CLI's stream-json protocol.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import shutil
from dataclasses import dataclass, field
from pathlib import Path
from typing import Awaitable, Callable, Optional

from .prompts import assemble_full_prompt

logger = logging.getLogger(__name__)


CURSOR_BIN = "cursor-agent"

# Conservative upper bound for what the bubble can hold before truncation
# (matches Telegram's 4096 limit minus a safety margin handled in bubble.py).
RENDER_BUDGET = 3500
# How much of the thinking buffer to surface in the bubble; the rest is dropped
# from the head, keeping the latest reasoning visible.
THINKING_WINDOW = 2400


UpdateCb = Callable[[str], Awaitable[None]]
FinalCb = Callable[[str], Awaitable[None]]


@dataclass
class _StreamState:
    thinking: str = ""
    assistant: str = ""
    result: Optional[str] = None
    is_error: bool = False
    stderr_tail: str = ""
    session_id: Optional[str] = None
    extra_lines: list[str] = field(default_factory=list)


def _render(state: _StreamState) -> str:
    """Render the live bubble snapshot from the current stream state."""
    parts: list[str] = []
    if state.thinking:
        chunk = state.thinking
        if len(chunk) > THINKING_WINDOW:
            chunk = "…" + chunk[-THINKING_WINDOW:]
        parts.append("thinking…\n" + chunk)
    if state.assistant:
        parts.append("answer:\n" + state.assistant)
    if not parts:
        parts.append("thinking…")
    text = "\n\n".join(parts)
    if len(text) > RENDER_BUDGET:
        text = text[-RENDER_BUDGET:]
    return text


def _render_final(state: _StreamState) -> str:
    if state.result and state.result.strip():
        return state.result.strip()
    if state.assistant.strip():
        return state.assistant.strip()
    if state.is_error:
        tail = state.stderr_tail.strip()
        return ("cursor-agent reported an error" + (f":\n{tail}" if tail else ""))
    return "(no output)"


class CursorRunner:
    def __init__(
        self,
        *,
        prompt: str,
        workdir: Path,
        model: Optional[str] = None,
        extra_env: Optional[dict[str, str]] = None,
        plan_mode: bool = False,
        system_prompt: Optional[str] = None,
        extra_prompts: Optional[str] = None,
        resume_session: Optional[str] = None,
    ) -> None:
        self.prompt = prompt
        self.workdir = workdir
        self.model = model
        self.extra_env = extra_env or {}
        self.plan_mode = plan_mode
        self.system_prompt = system_prompt
        self.extra_prompts = extra_prompts
        self.resume_session = resume_session
        # Populated after run(): the chat session id cursor-agent reported on
        # this invocation (== resume_session on a successful resume, a fresh
        # uuid on a brand-new chat, None if the run never reached a `system`
        # event).
        self.session_id: Optional[str] = None
        # Set True after run() if `resume_session` was supplied but the
        # invocation produced no events and exited non-zero -- almost always
        # "session not found" upstream. Caller should clear its stored id and
        # retry without --resume.
        self.resume_failed: bool = False

    def _build_cmd(self) -> list[str]:
        cmd = [
            CURSOR_BIN,
            "-p",
            "--output-format", "stream-json",
            "--stream-partial-output",
            "--force",
            "--workspace", str(self.workdir),
        ]
        if self.resume_session:
            cmd += ["--resume", self.resume_session]
        if self.plan_mode:
            cmd += ["--mode", "plan"]
        if self.model:
            cmd += ["--model", self.model]
        if self.system_prompt or self.extra_prompts:
            final = assemble_full_prompt(
                self.system_prompt or "",
                self.extra_prompts or "",
                self.prompt,
            )
        else:
            final = self.prompt
        cmd.append(final)
        return cmd

    async def run(self, *, on_update: UpdateCb, on_final: FinalCb) -> None:
        if shutil.which(CURSOR_BIN) is None:
            await on_final(f"`{CURSOR_BIN}` not found on this machine; install it first.")
            return

        try:
            self.workdir.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            await on_final(f"could not create workdir {self.workdir}: {exc}")
            return

        env = os.environ.copy()
        env.update(self.extra_env)

        cmd = self._build_cmd()
        logger.info("spawning cursor-agent: cwd=%s model=%s", self.workdir, self.model)

        try:
            proc = await asyncio.create_subprocess_exec(
                *cmd,
                cwd=str(self.workdir),
                env=env,
                stdin=asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
        except FileNotFoundError:
            await on_final(f"`{CURSOR_BIN}` not found on PATH.")
            return
        except OSError as exc:
            await on_final(f"failed to spawn cursor-agent: {exc}")
            return

        state = _StreamState()
        stderr_task = asyncio.create_task(self._drain_stderr(proc.stderr, state))
        try:
            await self._consume_stdout(proc.stdout, state, on_update)
            rc = await proc.wait()
        except asyncio.CancelledError:
            proc.terminate()
            try:
                await asyncio.wait_for(proc.wait(), timeout=5.0)
            except asyncio.TimeoutError:
                proc.kill()
                await proc.wait()
            raise
        finally:
            stderr_task.cancel()
            try:
                await stderr_task
            except (asyncio.CancelledError, Exception):
                pass

        # Surface cursor-agent's chat id to the caller so the agent layer
        # can persist it for future --resume calls.
        self.session_id = state.session_id

        # Heuristic: if we asked for --resume <id> and cursor-agent gave us
        # back nothing usable (no result, no session, non-zero exit), treat
        # it as a stale/missing session so the caller can clear its stored
        # id and retry without --resume.
        if (
            self.resume_session
            and rc != 0
            and not state.result
            and state.session_id is None
        ):
            self.resume_failed = True

        if rc != 0 and not state.result:
            state.is_error = True
            tail = state.stderr_tail.strip()
            state.result = (
                f"cursor-agent exited with code {rc}"
                + (f":\n{tail}" if tail else "")
            )
        await on_final(_render_final(state))

    async def _consume_stdout(
        self,
        stream: Optional[asyncio.StreamReader],
        state: _StreamState,
        on_update: UpdateCb,
    ) -> None:
        if stream is None:
            return
        while True:
            line = await stream.readline()
            if not line:
                return
            try:
                event = json.loads(line.decode("utf-8", errors="replace"))
            except json.JSONDecodeError:
                continue
            if not isinstance(event, dict):
                continue

            if self._apply_event(event, state):
                try:
                    await on_update(_render(state))
                except Exception:
                    logger.exception("on_update callback raised")

    def _apply_event(self, event: dict, state: _StreamState) -> bool:
        """Mutate ``state`` from one stream event. Return True if the bubble
        should be re-rendered."""
        etype = event.get("type")
        if etype == "system":
            sid = event.get("session_id")
            if sid:
                state.session_id = sid
            return False

        if etype == "thinking":
            sub = event.get("subtype")
            text = event.get("text") or ""
            if sub == "delta" and text:
                state.thinking += text
                return True
            if sub == "completed":
                return False
            return False

        if etype == "assistant":
            msg = event.get("message") or {}
            content = msg.get("content") or []
            buf = []
            for part in content:
                if isinstance(part, dict) and part.get("type") == "text":
                    buf.append(part.get("text") or "")
            full = "".join(buf).lstrip("\n")
            if full and full != state.assistant:
                state.assistant = full
                return True
            return False

        if etype == "result":
            res = event.get("result")
            if isinstance(res, str):
                state.result = res
            state.is_error = bool(event.get("is_error"))
            return False

        if etype == "user":
            return False

        return False

    async def _drain_stderr(
        self,
        stream: Optional[asyncio.StreamReader],
        state: _StreamState,
    ) -> None:
        if stream is None:
            return
        try:
            while True:
                chunk = await stream.read(4096)
                if not chunk:
                    return
                text = chunk.decode("utf-8", errors="replace")
                # Bumped from debug to info: when the bot reports "(no
                # output)" or a crash, this stderr is the only ground truth
                # for what cursor-agent actually said.
                for line in text.splitlines():
                    if line.strip():
                        logger.info("cursor-agent stderr: %s", line.rstrip())
                state.stderr_tail = (state.stderr_tail + text)[-2000:]
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("stderr drain crashed")
