"""Always-on per-machine cursor-agent backend.

Long-running process supervised by PM2 under the name ``arbos-<machine>``.
Long-polls the Telegram Bot API and, for every non-bot text message in this
machine's forum topic, spawns ``cursor-agent`` (in agent / print mode) inside
the install root (the directory where the repo is checked out). The agent's
``thinking`` deltas and assistant output are streamed back into a single
Telegram message bubble (edited at most once per second), and the bubble is
finally replaced with the agent's summary text once the run finishes.

Multiple incoming messages are handled in parallel: each message gets its
own bubble and its own cursor-agent subprocess.

Single-instance is enforced (in increasing order of strength):

1. PM2's unique-name constraint at the supervisor layer.
2. ``fcntl.flock`` on ``<install-root>/.arbos/agent.lock`` here, so a stray
   ``arbos agent run`` outside PM2 cannot double-poll.
3. Telegram's own ``409 Conflict`` on ``getUpdates`` if anyone slips past
   the first two -- we log + back off, never stealing updates.
"""

from __future__ import annotations

import asyncio
import fcntl
import logging
import os
import signal
from pathlib import Path
from typing import Any, Optional

import httpx

from .bubble import Bubble
from .config import StoredConfig
from .cursor_runner import CursorRunner
from .workspace import InstallPaths

logger = logging.getLogger(__name__)


BOT_API = "https://api.telegram.org"
LONG_POLL_TIMEOUT = 60        # seconds; matches Telegram's max
HTTP_TIMEOUT = LONG_POLL_TIMEOUT + 10
CONFLICT_BACKOFF = 5.0
ERROR_BACKOFF = 2.0

# The Telegram client library encodes message ids as server_id << 20 plus
# local-id bits, while the Bot API exposes the raw server message id. Forum
# topic_id is just the topic creator message's id, so we must right-shift the
# library's value to compare it against `message_thread_id` returned by
# getUpdates / sendMessage. See https://core.telegram.org/tdlib (message-id
# semantics).
_ARBOS_MSG_ID_SHIFT = 20

INITIAL_BUBBLE_TEXT = "thinking…"
PLAN_INITIAL_BUBBLE_TEXT = "planning…"
IMPL_INITIAL_BUBBLE_TEXT = "implementing…"

PLAN_MODEL = "claude-opus-4-7-high"
IMPL_MODEL = "claude-opus-4-7-high"

PLAN_PROMPT_PREAMBLE = (
    "You are in plan mode. Produce a complete, actionable plan for the user "
    "request below. Do NOT ask clarifying questions; make reasonable "
    "assumptions and state them explicitly. End with a concise numbered step "
    "list that another agent can execute verbatim.\n\nRequest:\n"
)

IMPL_PROMPT_PREAMBLE = (
    "Execute the following plan exactly. Do NOT ask any clarifying questions; "
    "if something is ambiguous, make a reasonable choice and continue. When "
    "finished, output a short summary of what changed.\n\n"
)


def _to_bot_thread_id(arbos_topic_id: int) -> int:
    return arbos_topic_id >> _ARBOS_MSG_ID_SHIFT


class SingleInstanceError(RuntimeError):
    """Raised when another agent process already holds the lock file."""


class CursorAgent:
    def __init__(self, paths: InstallPaths) -> None:
        self.paths = paths
        self.config = StoredConfig.read(paths.config_path)
        self.bot_token = self.config.bot.token
        self.chat_id = self.config.supergroup.chat_id
        # config.machine.topic_id is the encoded form (shifted left 20);
        # convert to the Bot API's raw server message id for filter+reply.
        self.topic_id = _to_bot_thread_id(self.config.machine.topic_id)
        self.machine = self.config.machine.name
        self.bot_user_id = self.config.bot.user_id
        self.bot_username = (self.config.bot.username or "").lstrip("@")
        self.workdir: Path = paths.root
        self.offset_path = paths.offset_path
        self.lock_path = paths.lock_path
        self._stop = asyncio.Event()
        self._lock_fd: Optional[int] = None
        self._tasks: set[asyncio.Task[None]] = set()

    # ---- single-instance lock --------------------------------------------
    def acquire_lock(self) -> None:
        self.lock_path.parent.mkdir(parents=True, exist_ok=True)
        fd = os.open(self.lock_path, os.O_RDWR | os.O_CREAT, 0o600)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            os.close(fd)
            raise SingleInstanceError(
                f"another agent already holds {self.lock_path}; refusing to start"
            ) from exc
        os.ftruncate(fd, 0)
        os.write(fd, f"{os.getpid()}\n".encode())
        self._lock_fd = fd

    def release_lock(self) -> None:
        if self._lock_fd is not None:
            try:
                fcntl.flock(self._lock_fd, fcntl.LOCK_UN)
            finally:
                os.close(self._lock_fd)
                self._lock_fd = None

    # ---- offset persistence ----------------------------------------------
    def _load_offset(self) -> int:
        try:
            return int(self.offset_path.read_text().strip() or "0")
        except (OSError, ValueError):
            return 0

    def _save_offset(self, offset: int) -> None:
        try:
            self.offset_path.write_text(str(offset))
        except OSError as exc:
            logger.warning("could not persist offset: %s", exc)

    # ---- HTTP helpers ----------------------------------------------------
    async def _api(self, http: httpx.AsyncClient, method: str, **params: Any) -> httpx.Response:
        url = f"{BOT_API}/bot{self.bot_token}/{method}"
        return await http.post(url, json=params)

    async def _drain_backlog(self, http: httpx.AsyncClient) -> int:
        """Skip any queued updates so a fresh start doesn't re-handle backlog."""
        resp = await self._api(http, "getUpdates", timeout=0, allowed_updates=["message"])
        if resp.status_code != 200:
            return 0
        payload = resp.json()
        if not payload.get("ok"):
            return 0
        updates = payload.get("result", [])
        if not updates:
            return 0
        last = max(u["update_id"] for u in updates) + 1
        logger.info("draining %d backlog update(s); offset -> %d", len(updates), last)
        return last

    # ---- message filtering -----------------------------------------------
    def _is_for_us(self, message: dict[str, Any]) -> bool:
        chat = message.get("chat") or {}
        if chat.get("id") != self.chat_id:
            return False
        if message.get("message_thread_id") != self.topic_id:
            return False
        sender = message.get("from") or {}
        if sender.get("is_bot"):
            return False
        if self.bot_user_id is not None and sender.get("id") == self.bot_user_id:
            return False
        text = (message.get("text") or "").strip()
        if not text:
            return False
        return True

    # ---- per-message handler ---------------------------------------------
    async def _send_initial_bubble(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        *,
        text: str = INITIAL_BUBBLE_TEXT,
    ) -> Optional[dict[str, Any]]:
        params = {
            "chat_id": self.chat_id,
            "message_thread_id": self.topic_id,
            "text": text,
            "disable_web_page_preview": True,
            "reply_parameters": {
                "message_id": message["message_id"],
                "allow_sending_without_reply": True,
            },
        }
        resp = await self._api(http, "sendMessage", **params)
        if resp.status_code != 200:
            logger.warning("sendMessage HTTP %d: %s", resp.status_code, resp.text[:200])
            return None
        body = resp.json()
        if not body.get("ok"):
            logger.warning("sendMessage not ok: %r", body)
            return None
        return body.get("result")

    def _match_plan_command(self, prompt: str) -> Optional[str]:
        """Return the trailing argument if ``prompt`` is a ``/plan`` command, else None."""
        head, _, rest = prompt.partition(" ")
        head_lower = head.lower()
        candidates = {"/plan"}
        if self.bot_username:
            candidates.add(f"/plan@{self.bot_username.lower()}")
        if head_lower not in candidates:
            return None
        return rest.strip()

    async def _handle_message(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        prompt = (message.get("text") or "").strip()
        sender = (message.get("from") or {}).get("username") or (message.get("from") or {}).get("id")

        plan_arg = self._match_plan_command(prompt)
        if plan_arg is not None:
            logger.info("dispatching /plan from %s (len=%d)", sender, len(plan_arg))
            await self._handle_plan_command(http, message, plan_arg)
            return

        logger.info("dispatching message from %s -> cursor-agent (len=%d)", sender, len(prompt))

        sent = await self._send_initial_bubble(http, message)
        if sent is None:
            return

        bubble = Bubble(
            http=http,
            bot_token=self.bot_token,
            chat_id=self.chat_id,
            message_thread_id=self.topic_id,
            message_id=sent["message_id"],
        )

        runner = CursorRunner(
            prompt=prompt,
            workdir=self.workdir,
            model="claude-opus-4-7-high",
        )
        try:
            await runner.run(on_update=bubble.update, on_final=bubble.finalize)
        except asyncio.CancelledError:
            try:
                await bubble.finalize("(interrupted)")
            finally:
                await bubble.aclose()
            raise
        except Exception as exc:
            logger.exception("cursor-agent run crashed")
            try:
                await bubble.finalize(f"cursor-agent crashed: {exc}")
            except Exception:
                pass
        finally:
            await bubble.aclose()

    async def _handle_plan_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        request: str,
    ) -> None:
        if not request:
            sent = await self._send_initial_bubble(
                http, message, text="usage: /plan <what you want done>"
            )
            if sent is not None:
                bubble = Bubble(
                    http=http,
                    bot_token=self.bot_token,
                    chat_id=self.chat_id,
                    message_thread_id=self.topic_id,
                    message_id=sent["message_id"],
                )
                await bubble.aclose()
            return

        # --- Phase 1: plan ------------------------------------------------
        sent1 = await self._send_initial_bubble(
            http, message, text=PLAN_INITIAL_BUBBLE_TEXT
        )
        if sent1 is None:
            return

        bubble1 = Bubble(
            http=http,
            bot_token=self.bot_token,
            chat_id=self.chat_id,
            message_thread_id=self.topic_id,
            message_id=sent1["message_id"],
        )

        plan_runner = CursorRunner(
            prompt=PLAN_PROMPT_PREAMBLE + request,
            workdir=self.workdir,
            model=PLAN_MODEL,
            plan_mode=True,
        )
        plan_holder: dict[str, str] = {"text": "", "error": ""}

        async def _capture_plan(text: str) -> None:
            plan_holder["text"] = text
            await bubble1.finalize(text)

        try:
            await plan_runner.run(on_update=bubble1.update, on_final=_capture_plan)
        except asyncio.CancelledError:
            try:
                await bubble1.finalize("(interrupted)")
            finally:
                await bubble1.aclose()
            raise
        except Exception as exc:
            logger.exception("cursor-agent plan-mode run crashed")
            plan_holder["error"] = str(exc)
            try:
                await bubble1.finalize(f"cursor-agent crashed: {exc}")
            except Exception:
                pass
        finally:
            await bubble1.aclose()

        plan_text = plan_holder["text"].strip()
        # If the plan phase failed (errored out, no output, or returned the
        # runner's own error sentinel), don't proceed to implementation.
        if (
            plan_holder["error"]
            or not plan_text
            or plan_text.lower().startswith("cursor-agent")
            or plan_text == "(no output)"
        ):
            return

        # --- Phase 2: implement ------------------------------------------
        sent2 = await self._send_initial_bubble(
            http, message, text=IMPL_INITIAL_BUBBLE_TEXT
        )
        if sent2 is None:
            return

        bubble2 = Bubble(
            http=http,
            bot_token=self.bot_token,
            chat_id=self.chat_id,
            message_thread_id=self.topic_id,
            message_id=sent2["message_id"],
        )

        impl_prompt = (
            IMPL_PROMPT_PREAMBLE
            + f"Original request:\n{request}\n\nPlan:\n{plan_text}"
        )
        impl_runner = CursorRunner(
            prompt=impl_prompt,
            workdir=self.workdir,
            model=IMPL_MODEL,
        )
        try:
            await impl_runner.run(on_update=bubble2.update, on_final=bubble2.finalize)
        except asyncio.CancelledError:
            try:
                await bubble2.finalize("(interrupted)")
            finally:
                await bubble2.aclose()
            raise
        except Exception as exc:
            logger.exception("cursor-agent implementation run crashed")
            try:
                await bubble2.finalize(f"cursor-agent crashed: {exc}")
            except Exception:
                pass
        finally:
            await bubble2.aclose()

    def _spawn_handler(self, http: httpx.AsyncClient, message: dict[str, Any]) -> None:
        task = asyncio.create_task(self._handle_message(http, message))
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)

    # ---- main loop -------------------------------------------------------
    async def run(self) -> None:
        loop = asyncio.get_running_loop()
        for sig in (signal.SIGINT, signal.SIGTERM):
            try:
                loop.add_signal_handler(sig, self._stop.set)
            except NotImplementedError:
                pass

        offset = self._load_offset()
        logger.info(
            "cursor-agent online: machine=%s chat=%s topic=%s workdir=%s starting_offset=%s",
            self.machine, self.chat_id, self.topic_id, self.workdir, offset,
        )

        async with httpx.AsyncClient(timeout=HTTP_TIMEOUT) as http:
            if offset == 0:
                offset = await self._drain_backlog(http)
                self._save_offset(offset)

            while not self._stop.is_set():
                try:
                    resp = await self._api(
                        http,
                        "getUpdates",
                        offset=offset,
                        timeout=LONG_POLL_TIMEOUT,
                        allowed_updates=["message"],
                    )
                except httpx.HTTPError as exc:
                    logger.warning("getUpdates network error: %s", exc)
                    await self._sleep_unless_stopped(ERROR_BACKOFF)
                    continue

                if resp.status_code == 409:
                    logger.warning(
                        "getUpdates 409 Conflict (another poller live); backing off %.1fs",
                        CONFLICT_BACKOFF,
                    )
                    await self._sleep_unless_stopped(CONFLICT_BACKOFF)
                    continue
                if resp.status_code != 200:
                    logger.warning("getUpdates HTTP %d: %s", resp.status_code, resp.text[:200])
                    await self._sleep_unless_stopped(ERROR_BACKOFF)
                    continue

                payload = resp.json()
                if not payload.get("ok"):
                    logger.warning("getUpdates not ok: %r", payload)
                    await self._sleep_unless_stopped(ERROR_BACKOFF)
                    continue

                for upd in payload.get("result", []):
                    offset = upd["update_id"] + 1
                    msg = upd.get("message")
                    if msg and self._is_for_us(msg):
                        self._spawn_handler(http, msg)
                    self._save_offset(offset)

            await self._drain_tasks()

        logger.info("cursor-agent stopping (signal received)")

    async def _drain_tasks(self) -> None:
        if not self._tasks:
            return
        logger.info("cancelling %d in-flight handler(s)…", len(self._tasks))
        for task in list(self._tasks):
            task.cancel()
        await asyncio.gather(*self._tasks, return_exceptions=True)
        self._tasks.clear()

    async def _sleep_unless_stopped(self, seconds: float) -> None:
        try:
            await asyncio.wait_for(self._stop.wait(), timeout=seconds)
        except asyncio.TimeoutError:
            pass


async def run_agent(paths: InstallPaths) -> int:
    if not paths.config_path.exists():
        logger.error(
            "config not found at %s; run `./run.sh install` first",
            paths.config_path,
        )
        return 2

    agent = CursorAgent(paths)
    try:
        agent.acquire_lock()
    except SingleInstanceError as exc:
        logger.error(str(exc))
        return 3

    try:
        await agent.run()
        return 0
    finally:
        agent.release_lock()
