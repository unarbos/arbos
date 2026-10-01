"""Codex CLI backend for GPT-Live client delegation.

Runs `codex exec --json` non-interactively in `CODEX_CWD`, reads the JSONL event
stream, and returns the agent's final message. One thread is kept for the length
of a call, so "what did you just run?" and "try the other one" mean something;
a newer question cancels whatever is still running.

The flags matter. `codex exec` has no `--full-auto` — passing it makes the CLI
exit with a usage error before it does any work, and that usage text was what
the call read out when it was asked to go and look something up.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import shutil
from collections.abc import AsyncIterator
from typing import Optional

log = logging.getLogger("voice.codex")

DEFAULT_BIN = os.environ.get("CODEX_BIN", "codex")
DEFAULT_CWD = os.environ.get("CODEX_CWD") or os.path.expanduser("~")
# The caller is on a phone with no screen to read: the sandbox cannot stop to ask
# for approval, because there is nobody at a keyboard to give it. The voice
# server is the trust boundary here, not the CLI.
SANDBOX_ARGS = [
    "--dangerously-bypass-approvals-and-sandbox",
    "--skip-git-repo-check",
]
# How the answer should come back, given it is about to be spoken rather than
# read. Without this, Codex replies in markdown with backticks and bullet lists
# and the voice says the punctuation.
VOICE_BRIEF = (
    "You are answering out loud on a phone call. Do the work or run whatever you need on this "
    "machine first, then reply with at most two short spoken sentences. Plain speech only: no "
    "markdown, no backticks, no code fences, no bullet lists, no headings. Do not read commands, "
    "diffs or file contents aloud — say what they showed. If you could not find out, say so "
    "plainly in one sentence.\n\nRequest: "
)
# Long enough for real work, short enough that the call is not left in silence.
DEFAULT_TIMEOUT = float(os.environ.get("CODEX_TIMEOUT", "150"))

_FENCE = re.compile(r"```.*?```", re.S)
_MARKUP = re.compile(r"[`*_#>]+")


def _speakable(text: str) -> str:
    """Strip the markdown Codex writes anyway so the voice does not read it."""
    text = _FENCE.sub(" ", text)
    text = _MARKUP.sub("", text)
    text = re.sub(r"^\s*[-+]\s+", "", text, flags=re.M)
    return " ".join(text.split())


class CodexClient:
    """One running Codex turn at a time; cancel() kills the process."""

    def __init__(self, *, api_key: str = "", cwd: str = "", binary: str = "") -> None:
        self.api_key = api_key or os.environ.get("OPENAI_API_KEY", "")
        self.cwd = cwd or DEFAULT_CWD
        self.binary = binary or DEFAULT_BIN
        self._proc: Optional[asyncio.subprocess.Process] = None
        self._lock = asyncio.Lock()
        # The conversation so far, so a follow-up question lands in the same
        # thread instead of talking to a stranger.
        self._thread: Optional[str] = None
        self._own_login: Optional[bool] = None

    @property
    def available(self) -> bool:
        return bool(shutil.which(self.binary) or os.path.isfile(self.binary))

    def reset(self) -> None:
        """New call, new conversation."""
        self._thread = None

    async def cancel(self) -> None:
        proc = self._proc
        if proc is None or proc.returncode is not None:
            return
        try:
            proc.terminate()
            try:
                await asyncio.wait_for(proc.wait(), 3)
            except asyncio.TimeoutError:
                proc.kill()
        except ProcessLookupError:
            pass
        self._proc = None

    async def _logged_in(self) -> bool:
        """Whether Codex already has its own credentials.

        If it does, the phone's key is left out of the environment. Codex that is
        signed in with ChatGPT and handed an `OPENAI_API_KEY` switches to that
        key, and a key without the right access turns every delegation into an
        authentication error — on a machine that was working a moment earlier.
        """
        if self._own_login is not None:
            return self._own_login
        self._own_login = False
        try:
            proc = await asyncio.create_subprocess_exec(
                self.binary, "login", "status",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
            )
            out, _ = await asyncio.wait_for(proc.communicate(), 10)
            self._own_login = proc.returncode == 0 and b"logged in" in out.lower()
        except Exception:
            log.debug("codex login status failed", exc_info=True)
        log.info("codex has own login: %s", self._own_login)
        return self._own_login

    def _args(self, question: str) -> list[str]:
        if self._thread:
            # `exec resume` takes no -C; the working directory comes from the
            # child's cwd instead.
            return [self.binary, "exec", "resume", self._thread, "--json", *SANDBOX_ARGS, question]
        return [self.binary, "exec", "--json", *SANDBOX_ARGS, "-C", self.cwd, question]

    async def turn(self, question: str, *, timeout: float = DEFAULT_TIMEOUT) -> AsyncIterator[str]:
        """Ask Codex `question`; yield the answer it finished with."""
        async with self._lock:
            await self.cancel()
            if not self.available:
                yield "Codex is not installed on this machine."
                return
            question = question.strip()
            if not question:
                yield "I did not catch a question for Codex."
                return

            env = os.environ.copy()
            if self.api_key and not await self._logged_in():
                env["OPENAI_API_KEY"] = self.api_key
            env.setdefault("TERM", "dumb")
            env["CODEX_QUIET_MODE"] = "1"

            args = self._args(VOICE_BRIEF + question)
            log.info("codex turn cwd=%s thread=%s q=%r", self.cwd, self._thread or "new", question[:120])
            try:
                self._proc = await asyncio.create_subprocess_exec(
                    *args,
                    cwd=self.cwd,
                    env=env,
                    stdin=asyncio.subprocess.DEVNULL,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                    limit=4 * 1024 * 1024,
                )
            except FileNotFoundError:
                yield "Codex is not installed on this machine."
                return
            except Exception as exc:
                log.exception("codex spawn failed")
                yield f"Codex could not start: {exc}"
                return

            proc = self._proc
            assert proc.stdout is not None
            messages: list[str] = []
            commands = 0
            failure = ""
            try:
                while True:
                    try:
                        line = await asyncio.wait_for(proc.stdout.readline(), timeout)
                    except asyncio.TimeoutError:
                        log.warning("codex timed out after %.0fs", timeout)
                        await self.cancel()
                        failure = "That took too long, so I stopped it."
                        break
                    except (asyncio.LimitOverrunError, ValueError):
                        # One absurd line (a pasted file) must not end the turn.
                        continue
                    if not line:
                        break
                    event = self._event(line)
                    if event is None:
                        continue
                    kind = event.get("type")
                    if kind == "thread.started":
                        thread = event.get("thread_id")
                        if thread:
                            self._thread = thread
                    elif kind == "item.completed":
                        item = event.get("item") or {}
                        if item.get("type") == "agent_message":
                            text = str(item.get("text") or "").strip()
                            if text:
                                messages.append(text)
                        elif item.get("type") == "command_execution":
                            commands += 1
                    elif kind in ("turn.failed", "error"):
                        failure = self._failure(event)
            finally:
                if proc.returncode is None:
                    await self.cancel()
                else:
                    await proc.wait()

            # The last message is the answer; earlier ones are the running
            # commentary ("I'll check the hostname"), which nobody needs spoken.
            answer = _speakable(messages[-1]) if messages else ""
            if answer:
                log.info("codex answered after %d command(s): %r", commands, answer[:160])
                yield answer
                return

            if not failure:
                failure = await self._stderr_note(proc)
            log.warning("codex produced no answer: %s", failure)
            yield failure

    @staticmethod
    def _event(line: bytes) -> Optional[dict]:
        text = line.decode("utf-8", errors="replace").strip()
        if not text or not text.startswith("{"):
            return None
        try:
            event = json.loads(text)
        except ValueError:
            return None
        return event if isinstance(event, dict) else None

    @staticmethod
    def _failure(event: dict) -> str:
        for key in ("message", "error", "reason"):
            value = event.get(key)
            if isinstance(value, str) and value.strip():
                return f"That failed: {value.strip()[:160]}"
            if isinstance(value, dict):
                nested = value.get("message")
                if isinstance(nested, str) and nested.strip():
                    return f"That failed: {nested.strip()[:160]}"
        return "That failed on the machine."

    async def _stderr_note(self, proc: asyncio.subprocess.Process) -> str:
        """A short, sayable reason — never the CLI's usage text.

        A broken invocation used to be spoken verbatim, so the answer to "what
        machine are we on" was two hundred characters of argument parser output.
        """
        raw = b""
        if proc.stderr is not None:
            try:
                raw = await asyncio.wait_for(proc.stderr.read(2000), 2.0)
            except Exception:
                raw = b""
        detail = raw.decode("utf-8", errors="replace").strip()
        if detail:
            log.warning("codex stderr: %s", detail[:600])
        lowered = detail.lower()
        if "unexpected argument" in lowered or "usage:" in lowered:
            return "The agent on the machine is misconfigured, so I could not run that."
        if "auth" in lowered or "401" in lowered or "api key" in lowered:
            return "The agent on the machine is not signed in, so I could not run that."
        if "rate limit" in lowered or "429" in lowered:
            return "The agent hit a rate limit. Ask me again in a moment."
        first = next((line for line in detail.splitlines() if line.strip()), "")
        if first:
            return f"That did not work: {first.strip()[:140]}"
        return "I could not get an answer from the machine."
