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
# How much of the thinking buffer to surface in the bubble *while we have no
# assistant text yet*. Once the assistant starts speaking we hide thinking
# entirely so the bubble doesn't scroll backwards on every delta.
THINKING_WINDOW = 1200
# Keep at most this many recent tool-call lines in the live bubble. Each line
# is one shell command / read / grep with a short status. Older lines are
# trimmed from the head so the latest activity stays in view.
TOOL_LOG_WINDOW = 6
# Truncate any single rendered tool line to this many chars (commands or
# paths can be long; we just want a glanceable hint).
TOOL_LINE_MAX = 120

# asyncio.StreamReader's default line buffer is 64 KiB; cursor-agent's
# stream-json events embed the full running assistant text on every delta
# and easily exceed that on long chats, raising LimitOverrunError
# ("Separator is found, but chunk is longer than limit"). 16 MiB is well
# beyond any realistic single event but still bounded.
STDOUT_LINE_LIMIT = 16 * 1024 * 1024


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
    # Rolling log of the most recent tool calls (one short line each), most
    # recent at the bottom. Surfaced in the bubble so the user can see
    # what the agent is actually doing instead of staring at "thinking…".
    tool_log: list[str] = field(default_factory=list)
    # call_id -> the rendered "started" line index in tool_log, so we can
    # mutate it in-place when the matching `completed` event arrives
    # (instead of appending a duplicate line).
    _tool_index: dict[str, int] = field(default_factory=dict)


def _trim(s: str, n: int) -> str:
    s = s.replace("\n", " ").strip()
    return s if len(s) <= n else s[: n - 1] + "…"


def _summarise_tool_call(tc: dict) -> Optional[tuple[str, str]]:
    """Return ``(verb, detail)`` for a single ``tool_call.tool_call`` payload.

    ``verb`` is a short label (e.g. ``$``, ``read``, ``grep``, ``edit``);
    ``detail`` is a one-line human-readable hint (a command, a path, a
    pattern). Returns ``None`` if we don't recognise the shape -- the
    caller will skip rendering rather than dump a JSON blob.
    """
    if not isinstance(tc, dict):
        return None
    if "shellToolCall" in tc:
        args = (tc["shellToolCall"] or {}).get("args") or {}
        cmd = args.get("command") or ""
        if cmd:
            return ("$", _trim(cmd, TOOL_LINE_MAX))
        desc = (tc["shellToolCall"] or {}).get("description") or ""
        return ("$", _trim(desc, TOOL_LINE_MAX) if desc else "(shell)")
    if "readToolCall" in tc:
        args = (tc["readToolCall"] or {}).get("args") or {}
        return ("read", _trim(str(args.get("path") or "?"), TOOL_LINE_MAX))
    if "writeToolCall" in tc:
        args = (tc["writeToolCall"] or {}).get("args") or {}
        return ("write", _trim(str(args.get("path") or "?"), TOOL_LINE_MAX))
    if "editToolCall" in tc or "strReplaceToolCall" in tc:
        key = "editToolCall" if "editToolCall" in tc else "strReplaceToolCall"
        args = (tc[key] or {}).get("args") or {}
        return ("edit", _trim(str(args.get("path") or args.get("filePath") or "?"), TOOL_LINE_MAX))
    if "grepToolCall" in tc:
        args = (tc["grepToolCall"] or {}).get("args") or {}
        pat = args.get("pattern") or ""
        path = args.get("path") or ""
        detail = pat + (f"  in {path}" if path else "")
        return ("grep", _trim(detail or "?", TOOL_LINE_MAX))
    if "globToolCall" in tc:
        args = (tc["globToolCall"] or {}).get("args") or {}
        return ("glob", _trim(str(args.get("globPattern") or args.get("pattern") or "?"), TOOL_LINE_MAX))
    if "deleteToolCall" in tc:
        args = (tc["deleteToolCall"] or {}).get("args") or {}
        return ("delete", _trim(str(args.get("path") or "?"), TOOL_LINE_MAX))
    if "todoWriteToolCall" in tc:
        return ("todos", "(updated)")
    if "webSearchToolCall" in tc:
        args = (tc["webSearchToolCall"] or {}).get("args") or {}
        return ("web", _trim(str(args.get("searchTerm") or args.get("query") or "?"), TOOL_LINE_MAX))
    if "webFetchToolCall" in tc:
        args = (tc["webFetchToolCall"] or {}).get("args") or {}
        return ("fetch", _trim(str(args.get("url") or "?"), TOOL_LINE_MAX))
    if "taskToolCall" in tc:
        args = (tc["taskToolCall"] or {}).get("args") or {}
        return ("task", _trim(str(args.get("description") or "(subagent)"), TOOL_LINE_MAX))
    # Unknown shape: surface the bare key so the user at least sees activity.
    keys = [k for k in tc.keys() if k.endswith("ToolCall")]
    if keys:
        return (keys[0].removesuffix("ToolCall"), "")
    return None


def _render(state: _StreamState) -> str:
    """Render the live bubble snapshot from the current stream state.

    Layout (top -> bottom):
      [tool log, last few entries]
      [thinking buffer, only while assistant is silent]
      [assistant text, accumulated]
    """
    parts: list[str] = []
    if state.tool_log:
        recent = state.tool_log[-TOOL_LOG_WINDOW:]
        parts.append("\n".join(recent))
    # Hide the thinking spew once we have any visible answer text. Thinking
    # mid-answer just causes the bubble to scroll backwards on every delta
    # (Telegram only renders the head, not the tail, when content overflows).
    if state.thinking and not state.assistant:
        chunk = state.thinking
        if len(chunk) > THINKING_WINDOW:
            chunk = "…" + chunk[-THINKING_WINDOW:]
        parts.append("thinking…\n" + chunk)
    if state.assistant:
        parts.append(state.assistant)
    if not parts:
        parts.append("thinking…")
    text = "\n\n".join(parts)
    if len(text) > RENDER_BUDGET:
        # Keep the *tail* (latest answer text) when over budget. The user
        # cares about what's happening now, not the historical tool log.
        text = "…\n" + text[-(RENDER_BUDGET - 2) :]
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
                limit=STDOUT_LINE_LIMIT,
                # Run cursor-agent in its own session/process-group so that a
                # SIGINT delivered to *our* pgid (e.g. by `pm2 reload` or a
                # terminal Ctrl-C against the parent) does not also kill the
                # child mid-run. Without this the child would exit 130 and
                # the bubble would surface "cursor-agent exited with code
                # 130: Aborting operation..." every time we got reloaded.
                # We still cancel/terminate the child explicitly on our own
                # asyncio.CancelledError below, so a real shutdown still
                # tears it down promptly.
                start_new_session=True,
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
            # rc 130 == SIGINT, 143 == SIGTERM. These almost always mean the
            # parent agent was being shut down or reloaded (pm2 reload sends
            # SIGINT to our pid; older builds without start_new_session=True
            # propagated that to the child). Render a friendlier line so the
            # user doesn't see a scary "Aborting operation..." after every
            # reload.
            if rc in (130, -2, 143, -15):
                state.result = (
                    "cursor-agent run interrupted "
                    "(parent agent was reloaded or shut down). "
                    "Send your message again to retry."
                )
            else:
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
            try:
                line = await stream.readline()
            except asyncio.LimitOverrunError as exc:
                # Single NDJSON event exceeded our (16 MiB) line limit.
                # Drain it from the buffer so the stream keeps moving and
                # log a single warning -- we lose this one event but the
                # rest of the run continues normally.
                logger.warning(
                    "cursor-agent stdout line exceeded limit (%d bytes consumed); skipping",
                    exc.consumed,
                )
                try:
                    await stream.readexactly(exc.consumed)
                except (asyncio.IncompleteReadError, Exception):
                    return
                continue
            except (asyncio.IncompleteReadError, ValueError) as exc:
                logger.warning("cursor-agent stdout read failed: %s", exc)
                return
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
            chunk = "".join(buf)
            if not chunk:
                return False
            # cursor-agent with --stream-partial-output emits two flavours
            # of `assistant` event:
            #   * deltas:    have a `timestamp_ms`; `chunk` is just the
            #                latest token slice (e.g. " `hello`").
            #   * snapshot:  no `timestamp_ms`; `chunk` is the full text
            #                (sent once at end of each assistant turn).
            # Treating every event as a snapshot (the old behaviour)
            # caused the bubble to flicker between tiny token slivers.
            # Treating every event as a delta would double the final
            # text. We branch on `timestamp_ms` to do the right thing.
            is_delta = event.get("timestamp_ms") is not None
            if is_delta:
                # Strip leading newlines only on the *first* delta so the
                # bubble doesn't open with a blank gap.
                if not state.assistant:
                    chunk = chunk.lstrip("\n")
                if not chunk:
                    return False
                state.assistant += chunk
                return True
            # Full snapshot: only replace if it actually has more content
            # than what we accumulated from deltas (defensive: in case a
            # delta was lost or arrived out of order).
            chunk = chunk.lstrip("\n")
            if len(chunk) > len(state.assistant):
                state.assistant = chunk
                return True
            return False

        if etype == "tool_call":
            sub = event.get("subtype")
            tc = event.get("tool_call") or {}
            call_id = event.get("call_id") or ""
            summary = _summarise_tool_call(tc)
            if summary is None:
                return False
            verb, detail = summary
            if sub == "started":
                line = f"⏳ {verb} {detail}".rstrip()
                state.tool_log.append(line)
                if call_id:
                    state._tool_index[call_id] = len(state.tool_log) - 1
                return True
            if sub == "completed":
                ok = True
                # Best-effort: shellToolCall surfaces exit code; readToolCall
                # / others surface .result.error{...}. If we can't tell,
                # default to ok.
                for _key, payload in tc.items():
                    if not isinstance(payload, dict):
                        continue
                    res = payload.get("result")
                    if not isinstance(res, dict):
                        continue
                    if "error" in res:
                        ok = False
                        break
                    succ = res.get("success")
                    if isinstance(succ, dict) and succ.get("exitCode") not in (None, 0, "0"):
                        ok = False
                        break
                marker = "✓" if ok else "✗"
                new_line = f"{marker} {verb} {detail}".rstrip()
                idx = state._tool_index.pop(call_id, None) if call_id else None
                if idx is not None and 0 <= idx < len(state.tool_log):
                    state.tool_log[idx] = new_line
                else:
                    state.tool_log.append(new_line)
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
