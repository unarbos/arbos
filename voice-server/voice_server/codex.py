"""Codex CLI backend for GPT-Live client delegation.

Runs `codex exec` (or `CODEX_BIN`) non-interactively in `CODEX_CWD`, streams
stdout as answer deltas, and cancels the process when a newer question arrives.
"""

from __future__ import annotations

import asyncio
import logging
import os
import shutil
from collections.abc import AsyncIterator
from typing import Optional

log = logging.getLogger("voice.codex")

DEFAULT_BIN = os.environ.get("CODEX_BIN", "codex")
DEFAULT_CWD = os.environ.get("CODEX_CWD") or os.path.expanduser("~")
# Prefer a quiet, non-interactive exec. Override with CODEX_ARGS as a shell-ish string.
DEFAULT_ARGS = os.environ.get("CODEX_ARGS", "exec --full-auto")


class CodexClient:
    """One running Codex turn at a time; cancel() kills the process."""

    def __init__(self, *, api_key: str = "", cwd: str = "", binary: str = "") -> None:
        self.api_key = api_key or os.environ.get("OPENAI_API_KEY", "")
        self.cwd = cwd or DEFAULT_CWD
        self.binary = binary or DEFAULT_BIN
        self._proc: Optional[asyncio.subprocess.Process] = None
        self._lock = asyncio.Lock()

    @property
    def available(self) -> bool:
        return bool(shutil.which(self.binary) or os.path.isfile(self.binary))

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

    async def turn(self, question: str, *, timeout: float = 180) -> AsyncIterator[str]:
        """Ask Codex `question`; yield stdout chunks as they arrive."""
        async with self._lock:
            await self.cancel()
            if not self.available:
                yield "Codex is not installed on this machine."
                return
            if not question.strip():
                yield "I did not catch a question for Codex."
                return

            args = [self.binary, *DEFAULT_ARGS.split(), question.strip()]
            env = os.environ.copy()
            if self.api_key:
                env["OPENAI_API_KEY"] = self.api_key
            # Keep child quiet; we only want the answer text.
            env.setdefault("TERM", "dumb")
            log.info("codex turn cwd=%s bin=%s q=%r", self.cwd, self.binary, question[:120])
            try:
                self._proc = await asyncio.create_subprocess_exec(
                    *args,
                    cwd=self.cwd,
                    env=env,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                    limit=1024 * 1024,
                )
            except FileNotFoundError:
                yield "Codex is not installed on this machine."
                return
            except Exception as exc:
                log.exception("codex spawn failed")
                yield f"Codex failed to start: {exc}"
                return

            proc = self._proc
            assert proc.stdout is not None
            deadline = asyncio.get_event_loop().time() + timeout
            buf = b""
            try:
                while True:
                    remaining = deadline - asyncio.get_event_loop().time()
                    if remaining <= 0:
                        await self.cancel()
                        if not buf:
                            yield "Codex timed out."
                        break
                    try:
                        chunk = await asyncio.wait_for(proc.stdout.read(4096), min(remaining, 2.0))
                    except asyncio.TimeoutError:
                        if proc.returncode is not None:
                            break
                        continue
                    if not chunk:
                        break
                    buf += chunk
                    # Emit on newlines so GPT-Live gets readable pieces; flush remainder at end.
                    while b"\n" in buf:
                        line, buf = buf.split(b"\n", 1)
                        text = line.decode("utf-8", errors="replace").rstrip()
                        if text:
                            yield text + "\n"
                if buf:
                    text = buf.decode("utf-8", errors="replace").rstrip()
                    if text:
                        yield text
                # If Codex wrote only to stderr, surface a short note.
                if proc.returncode not in (0, None) and not buf:
                    err = b""
                    if proc.stderr is not None:
                        try:
                            err = await asyncio.wait_for(proc.stderr.read(800), 1.0)
                        except Exception:
                            err = b""
                    msg = err.decode("utf-8", errors="replace").strip() or f"exit {proc.returncode}"
                    log.warning("codex failed: %s", msg[:200])
                    yield f"Codex failed: {msg[:200]}"
            finally:
                if proc.returncode is None:
                    await self.cancel()
                self._proc = None
