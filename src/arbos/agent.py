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
import json
import logging
import os
import signal
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable, Optional

import httpx

from .bubble import Bubble
from .config import StoredConfig
from .cursor_runner import CursorRunner
from . import inflight
from .inflight import InflightEntry, InflightStore
from .outbox import OutboxWatcher
from .prompts import build_prompt, load_extra_prompts
from . import runjournal
from .runjournal import RunJournal
from . import session_store
from .session_store import ChatSession
from .supervisor import Supervisor
from . import updater
from .updater import UpdateError, UpdateResult
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
TRANSCRIBING_BUBBLE_TEXT = "transcribing…"

OPENAI_TRANSCRIPTIONS_URL = "https://api.openai.com/v1/audio/transcriptions"
WHISPER_MODEL = "whisper-1"
WHISPER_TIMEOUT = 60.0

PLAN_MODEL = "claude-opus-4-7-high"
IMPL_MODEL = "claude-opus-4-7-high"
CHAT_MODEL = "claude-opus-4-7-high"

# Supervisor layer: a small LLM throttles the worker's noisy snapshot stream
# down to a single short status line every few seconds, so the bubble stays
# glanceable mid-run. Set ARBOS_SUPERVISOR_DISABLE=1 (or omit OPENAI_API_KEY)
# to bypass and stream raw snapshots straight to the bubble (legacy behavior).
SUPERVISOR_MODEL = (
    os.environ.get("ARBOS_SUPERVISOR_MODEL", "gpt-4o-mini").strip()
    or "gpt-4o-mini"
)
SUPERVISOR_INTERVAL = 2.0
SUPERVISOR_MAX_CHARS = 250
SUPERVISOR_DISABLE = os.environ.get("ARBOS_SUPERVISOR_DISABLE", "").strip() == "1"

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

# How long a Confirm/Cancel bubble stays clickable. After this many seconds
# the in-memory pending entry is dropped and a click yields a "expired" toast.
CONFIRM_TTL_SECONDS = 300.0

# Set ARBOS_SKIP_CONFIRM=1 (in the agent's env, e.g. via run.sh or pm2 ecosystem)
# to bypass the auto-wrap of destructive ops. Useful for scripted replays.
SKIP_CONFIRM_ENV = "ARBOS_SKIP_CONFIRM"


@dataclass
class _PendingConfirm:
    """One outstanding Confirm/Cancel bubble waiting on a callback.

    Lives only in :attr:`CursorAgent._pending_confirms`; intentionally not
    journaled -- if the agent restarts before the user clicks, the entry is
    silently dropped and they re-issue.
    """

    bubble_message_id: int
    user_id: Optional[int]
    label: str
    original_message: dict[str, Any]
    action: Callable[[Bubble], Awaitable[None]]
    expires_at: float = field(default_factory=lambda: time.monotonic() + CONFIRM_TTL_SECONDS)


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
        # Logical machine name from config.json -- locked at install time,
        # drives Telegram routing (bot username, topic title) and stays
        # stable even if the host's tailscale name changes later.
        self.machine = self.config.machine.name
        # Physical pm2 entry name. ``run.sh`` derives ``ARBOS_MACHINE`` from
        # tailscale at start-up and exports it into the agent's env; the
        # config-time machine name is the fallback when ARBOS_MACHINE is
        # unset (e.g. running `arbos agent run` outside pm2). Without this
        # split, ``pm2 reload arbos-<config-name>`` from /restart and
        # /update silently no-ops on hosts whose pm2 entry is named after
        # the (different) tailscale label.
        env_machine = (os.environ.get("ARBOS_MACHINE") or "").strip()
        self.pm2_name = f"arbos-{env_machine or self.machine}"
        self.bot_user_id = self.config.bot.user_id
        self.bot_username = (self.config.bot.username or "").lstrip("@")
        self.workdir: Path = paths.root
        self.offset_path = paths.offset_path
        self.lock_path = paths.lock_path
        self.openai_api_key = os.environ.get("OPENAI_API_KEY", "").strip()
        if not self.openai_api_key:
            logger.warning(
                "OPENAI_API_KEY not set in env — voice transcription will be disabled"
            )
        self._stop = asyncio.Event()
        self._lock_fd: Optional[int] = None
        self._tasks: set[asyncio.Task[None]] = set()
        # Crash-recovery journal of in-flight Telegram updates. Written
        # before the offset advances and on every handler completion, so
        # a SIGINT mid-handler doesn't silently drop the user's question.
        # See ``inflight.py`` for the full lifecycle.
        self._inflight = InflightStore(self.paths)

        # Per-bubble run journal. Records the trigger + tool log + final
        # text for every bubble we post, keyed by Telegram message_id, so
        # that when the user replies to one of our messages we can decorate
        # the next cursor-agent prompt with grounded context. See
        # ``runjournal.py`` and :meth:`_build_reply_context_block`.
        self._runs = RunJournal(self.paths)

        # Persistent per-topic cursor-agent chat. Loaded from disk so memory
        # survives PM2 restarts. The lock serialises every spawn that touches
        # this chat -- two parallel `--resume <id>` calls would interleave
        # appends.
        loaded = session_store.load(self.paths)
        self._chat_session_id: Optional[str] = loaded.session_id if loaded else None
        self._chat_lock = asyncio.Lock()
        if loaded is not None:
            logger.info("loaded persistent chat session: %s", loaded.session_id)

        # In-memory map of bubble message_id -> _PendingConfirm. Populated by
        # the destructive-op auto-wrap and by `/confirm <command>`; consumed by
        # callback_query updates. Deliberately not persisted across restarts.
        self._pending_confirms: dict[int, _PendingConfirm] = {}
        self._skip_confirm = os.environ.get(SKIP_CONFIRM_ENV, "").strip() not in (
            "", "0", "false", "False", "no", "NO",
        )
        if self._skip_confirm:
            logger.info(
                "%s set; destructive ops will run without Confirm/Cancel prompt",
                SKIP_CONFIRM_ENV,
            )

        # Render the system preamble eagerly so PROMPT.md exists from t=0
        # even before the first message; build_prompt persists to disk and
        # caches in-process for ~60s.
        try:
            build_prompt(self.paths, self.config)
        except Exception:
            logger.exception("initial PROMPT.md render failed; continuing")

    def _system_prompt(self) -> str:
        """Cached system preamble (refreshed by build_prompt's own TTL)."""
        try:
            return build_prompt(self.paths, self.config)
        except Exception:
            logger.exception("system prompt render failed; sending empty preamble")
            return ""

    def _extra_prompts(self) -> str:
        try:
            return load_extra_prompts(self.paths)
        except Exception:
            logger.exception("extra prompts load failed; skipping")
            return ""

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
        resp = await self._api(
            http,
            "getUpdates",
            timeout=0,
            allowed_updates=["message", "callback_query"],
        )
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
        if (message.get("text") or "").strip():
            return True
        if self._extract_audio(message) is not None:
            return True
        return False

    @staticmethod
    def _extract_audio(message: dict[str, Any]) -> Optional[dict[str, Any]]:
        """Return audio metadata for the first supported attachment, else None.

        Preference order: voice (the canonical "voice note") > video_note
        (round video; audio track is what we transcribe) > audio (uploaded
        music file). Documents intentionally skipped.
        """
        for kind in ("voice", "video_note", "audio"):
            att = message.get(kind)
            if not att:
                continue
            file_id = att.get("file_id")
            if not file_id:
                continue
            return {
                "kind": kind,
                "file_id": file_id,
                "mime_type": att.get("mime_type") or "",
                "duration": att.get("duration") or 0,
            }
        return None

    # ---- per-message handler ---------------------------------------------
    async def _send_initial_bubble(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        *,
        text: str = INITIAL_BUBBLE_TEXT,
        reply_markup: Optional[dict[str, Any]] = None,
    ) -> Optional[dict[str, Any]]:
        params: dict[str, Any] = {
            "chat_id": self.chat_id,
            "message_thread_id": self.topic_id,
            "text": text,
            "disable_web_page_preview": True,
            "reply_parameters": {
                "message_id": message["message_id"],
                "allow_sending_without_reply": True,
            },
        }
        if reply_markup is not None:
            params["reply_markup"] = reply_markup
        resp = await self._api(http, "sendMessage", **params)
        if resp.status_code != 200:
            logger.warning("sendMessage HTTP %d: %s", resp.status_code, resp.text[:200])
            return None
        body = resp.json()
        if not body.get("ok"):
            logger.warning("sendMessage not ok: %r", body)
            return None
        return body.get("result")

    def _make_bubble(
        self,
        http: httpx.AsyncClient,
        message_id: int,
        *,
        chat_id: Optional[int] = None,
        message_thread_id: Optional[int] = None,
    ) -> Bubble:
        """Wrap a Telegram message id in a ``Bubble`` with our journal hook.

        Centralised so every Bubble in the agent automatically rolls its
        run-journal entry onto each continuation message_id when a long
        answer overflows Telegram's 4096-char per-message cap. Callers
        that override ``chat_id`` / ``message_thread_id`` are the rare
        cross-thread bubbles (e.g. confirmations posted into a different
        topic); everyone else uses this install's primary topic.
        """

        async def _on_continuation(prev_id: int, new_id: int) -> None:
            # Mirror the journal entry onto the new message id so a
            # reply that lands on either part resolves to the same run.
            try:
                self._runs.link_alias(prev_id, new_id)
            except Exception:
                logger.exception("runjournal link_alias failed (swallowed)")

        return Bubble(
            http=http,
            bot_token=self.bot_token,
            chat_id=chat_id if chat_id is not None else self.chat_id,
            message_thread_id=(
                message_thread_id if message_thread_id is not None else self.topic_id
            ),
            message_id=message_id,
            on_continuation=_on_continuation,
        )

    def _make_supervisor(self, bubble: Bubble, initial_text: str) -> Supervisor:
        """Wrap ``bubble`` in a Supervisor that summarises the worker's noisy
        snapshot stream into a single short status line every couple of
        seconds. When the supervisor is disabled (no key, or
        ``ARBOS_SUPERVISOR_DISABLE=1``), the wrapper passes worker snapshots
        straight through so behaviour matches the legacy direct-bubble path.
        """
        key = "" if SUPERVISOR_DISABLE else self.openai_api_key
        return Supervisor(
            bubble=bubble,
            openai_api_key=key,
            model=SUPERVISOR_MODEL,
            interval=SUPERVISOR_INTERVAL,
            max_chars=SUPERVISOR_MAX_CHARS,
            initial_text=initial_text,
        )

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

    def _match_reset_command(self, prompt: str) -> bool:
        """True if ``prompt`` is a bare ``/reset`` or ``/new`` (with optional
        ``@<bot>`` suffix). Trailing arguments after the command are ignored
        but still treated as a reset request."""
        head = prompt.partition(" ")[0].lower()
        candidates = {"/reset", "/new"}
        if self.bot_username:
            uname = self.bot_username.lower()
            candidates.add(f"/reset@{uname}")
            candidates.add(f"/new@{uname}")
        return head in candidates

    def _match_update_command(self, prompt: str) -> bool:
        """True if ``prompt`` is ``/update`` (optionally ``@<bot>``).

        Trailing arguments are ignored -- /update always pulls origin/main
        per the project's update policy.
        """
        head = prompt.partition(" ")[0].lower()
        candidates = {"/update"}
        if self.bot_username:
            candidates.add(f"/update@{self.bot_username.lower()}")
        return head in candidates

    def _match_restart_command(self, prompt: str) -> bool:
        """True if ``prompt`` is ``/restart`` (optionally ``@<bot>``).

        Trailing arguments are ignored -- /restart is parameterless.
        """
        head = prompt.partition(" ")[0].lower()
        candidates = {"/restart"}
        if self.bot_username:
            candidates.add(f"/restart@{self.bot_username.lower()}")
        return head in candidates

    def _match_confirm_command(self, prompt: str) -> Optional[str]:
        """Return the trailing argument if ``prompt`` is ``/confirm``, else None.

        Lets the user explicitly gate any other prompt or command behind a
        Confirm/Cancel bubble. Empty argument is treated as a usage error
        upstream.
        """
        head, _, rest = prompt.partition(" ")
        head_lower = head.lower()
        candidates = {"/confirm"}
        if self.bot_username:
            candidates.add(f"/confirm@{self.bot_username.lower()}")
        if head_lower not in candidates:
            return None
        return rest.strip()

    def _trigger_sender_label(self, message: dict[str, Any]) -> str:
        """Short identifier for the user who sent ``message`` (for the journal)."""
        sender = message.get("from") or {}
        return str(
            sender.get("username")
            or sender.get("first_name")
            or sender.get("id")
            or ""
        )

    def _journal_command_start(
        self,
        bubble_msg_id: int,
        *,
        kind: str,
        message: Optional[dict[str, Any]],
        trigger_text: str,
    ) -> None:
        """Open a journal entry for a parameterless command bubble.

        Best-effort and exception-safe -- swallows any failure so a
        journal hiccup never breaks the command itself.
        """
        try:
            sender_label = (
                self._trigger_sender_label(message) if message else ""
            )
            trigger_id = message.get("message_id") if message else None
            self._runs.record_start(
                bubble_msg_id,
                kind=kind,
                trigger_message_id=trigger_id,
                trigger_text=trigger_text,
                trigger_sender=sender_label,
            )
        except Exception:
            logger.exception("runjournal record_start (%s) crashed", kind)

    def _journal_command_finish(
        self,
        bubble_msg_id: int,
        *,
        status: str,
        final_text: str,
    ) -> None:
        """Close out a parameterless-command journal entry."""
        try:
            self._runs.record_finish(
                bubble_msg_id,
                status=status,
                final_text=final_text,
                tool_log=[],
            )
        except Exception:
            logger.exception("runjournal record_finish crashed")

    def _build_reply_context_block(
        self,
        message: dict[str, Any],
        prompt: str,
    ) -> str:
        """Render a ``<<<REPLY_CONTEXT>>>`` block for replies to our bubbles.

        Returns ``""`` when:

        * the message is not a Telegram reply, or
        * the reply target was not posted by *our* bot, or
        * the replier is not the original sender of the parent run (avoids
          cross-user context leakage in shared topics).

        On a successful lookup, summarises the prior run via
        :func:`runjournal.summarise` and wraps it in fences identical in
        style to ``<<<USER_MESSAGE>>>`` so the model treats it as
        instructions rather than user text.
        """
        try:
            reply = message.get("reply_to_message")
            if not isinstance(reply, dict):
                return ""
            reply_from = reply.get("from") or {}
            if self.bot_user_id is None:
                return ""
            if reply_from.get("id") != self.bot_user_id:
                # Reply to a human message; not ours to annotate.
                return ""
            reply_msg_id = reply.get("message_id")
            if not isinstance(reply_msg_id, int):
                return ""

            rec = self._runs.get(reply_msg_id)
            if rec is None:
                # Bubble exists but we have no provenance (older than our
                # journal, or written before the journal existed). Surface
                # what Telegram itself gave us so the model knows roughly
                # what the user is pointing at.
                quoted = (reply.get("text") or "").strip()
                quoted = quoted if len(quoted) <= 1500 else quoted[:1499] + "…"
                if not quoted:
                    return ""
                body = (
                    "The user is replying to one of my earlier Telegram bubbles, "
                    "but I no longer have provenance for it (predates my run "
                    "journal or was lost on restart).\n\n"
                    f'The bubble said:\n"""\n{quoted}\n"""'
                )
            else:
                body = (
                    "The user is replying to a previous Arbos message in this "
                    "same Telegram topic. Here is the run that produced that "
                    "bubble:\n\n"
                    + runjournal.summarise(rec)
                    + "\n\n"
                    + 'If they say "finished?", "no", "do that again", "what '
                    'about the second one", etc., resolve the referent against '
                    "this run before answering."
                )

                # If it's a /plan or /impl bubble and we have the paired
                # half (plan <-> impl), include that too -- the user
                # almost certainly cares about the linked phase as well.
                if rec.paired_bubble_id is not None:
                    paired = self._runs.get(rec.paired_bubble_id)
                    if paired is not None:
                        body += (
                            "\n\nLinked phase:\n"
                            + runjournal.summarise(paired)
                        )

            return (
                "<<<REPLY_CONTEXT>>>\n"
                + body.rstrip()
                + "\n<<<END_REPLY_CONTEXT>>>"
            )
        except Exception:
            logger.exception("reply-context build failed; continuing without it")
            return ""

    async def _handle_message(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        prompt = (message.get("text") or "").strip()
        sender = (message.get("from") or {}).get("username") or (message.get("from") or {}).get("id")

        # Voice / video_note / audio: transcribe before dispatch. The
        # transcribing bubble is reused as the live bubble for the cursor
        # run when the message routes to the regular agent path; for /plan
        # we finalize it with the heard transcript so the user can see it.
        voice_bubble: Optional[Bubble] = None
        if not prompt:
            audio = self._extract_audio(message)
            if audio is None:
                logger.debug("ignoring message with no text and no audio")
                return

            sent = await self._send_initial_bubble(
                http, message, text=TRANSCRIBING_BUBBLE_TEXT
            )
            if sent is None:
                return
            voice_bubble = self._make_bubble(http, sent["message_id"])

            try:
                prompt = await self._transcribe_voice(http, audio)
            except Exception as exc:
                logger.warning("voice transcription failed: %s", exc)
                try:
                    await voice_bubble.finalize(f"voice transcription failed: {exc}")
                finally:
                    await voice_bubble.aclose()
                return

            if not prompt:
                try:
                    await voice_bubble.finalize("voice transcription returned empty text")
                finally:
                    await voice_bubble.aclose()
                return

            logger.info(
                "transcribed %s from %s -> %d chars",
                audio["kind"], sender, len(prompt),
            )

        # If this message is a Telegram reply to one of *our* bubbles, look
        # up the run journal entry and build a REPLY_CONTEXT block. Empty
        # string when not a reply (or not a reply to us). Only the chat +
        # /plan paths consume it; the parameterless commands (/update,
        # /restart, /reset, /confirm) ignore it intentionally.
        reply_block = self._build_reply_context_block(message, prompt)
        if reply_block:
            logger.info(
                "reply-context attached for %s (block=%d chars)",
                sender, len(reply_block),
            )

        plan_arg = self._match_plan_command(prompt)
        if plan_arg is not None:
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            logger.info("dispatching /plan from %s (len=%d)", sender, len(plan_arg))
            await self._handle_plan_command(
                http, message, plan_arg, reply_context=reply_block
            )
            return

        confirm_arg = self._match_confirm_command(prompt)
        if confirm_arg is not None:
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            logger.info("dispatching /confirm from %s (wraps: %r)", sender, confirm_arg[:60])
            await self._handle_confirm_command(http, message, confirm_arg)
            return

        if self._match_reset_command(prompt):
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            if self._skip_confirm:
                logger.info("dispatching /reset from %s (confirm skipped)", sender)
                await self._handle_reset_command(http, message)
            else:
                logger.info("gating /reset from %s behind Confirm/Cancel", sender)
                await self._prompt_confirmation(
                    http,
                    message,
                    label="/reset",
                    description="wipe the persistent chat session",
                    action=lambda b: self._handle_reset_action(b),
                )
            return

        if self._match_update_command(prompt):
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            if self._skip_confirm:
                logger.info("dispatching /update from %s (confirm skipped)", sender)
                await self._handle_update_command(http, message)
            else:
                logger.info("gating /update from %s behind Confirm/Cancel", sender)
                await self._prompt_confirmation(
                    http,
                    message,
                    label="/update",
                    description=(
                        "git fetch + hard-reset to origin/main, reinstall if "
                        "needed, then pm2 reload this machine"
                    ),
                    action=lambda b: self._handle_update_action(b),
                )
            return

        if self._match_restart_command(prompt):
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            if self._skip_confirm:
                logger.info("dispatching /restart from %s (confirm skipped)", sender)
                await self._handle_restart_command(http, message)
            else:
                logger.info("gating /restart from %s behind Confirm/Cancel", sender)
                await self._prompt_confirmation(
                    http,
                    message,
                    label="/restart",
                    description="pm2 reload this machine (no code changes)",
                    action=lambda b: self._handle_restart_action(b),
                )
            return

        logger.info("dispatching message from %s -> cursor-agent (len=%d)", sender, len(prompt))

        if voice_bubble is not None:
            bubble = voice_bubble
            await bubble.update(INITIAL_BUBBLE_TEXT)
        else:
            sent = await self._send_initial_bubble(http, message)
            if sent is None:
                return
            bubble = self._make_bubble(http, sent["message_id"])

        # Open a journal entry for this bubble *before* we hand off to the
        # runner, so the user can already reply to the in-flight bubble
        # and get a sensible "(run is still in progress)" context block.
        self._runs.record_start(
            bubble.message_id,
            kind="chat",
            trigger_message_id=message.get("message_id"),
            trigger_text=prompt,
            trigger_sender=self._trigger_sender_label(message),
            model=CHAT_MODEL,
        )

        await self._run_persistent_chat(
            prompt=prompt,
            bubble=bubble,
            reply_context=reply_block,
        )

    async def _run_persistent_chat(
        self,
        *,
        prompt: str,
        bubble: Bubble,
        reply_context: str = "",
    ) -> None:
        """Spawn cursor-agent against the per-topic persistent chat.

        Serialised on ``self._chat_lock`` because cursor-agent's ``--resume``
        appends to a single chat thread; concurrent appends would interleave.
        Auto-recovers from a stale/missing session by clearing the stored id
        and retrying once without ``--resume``.

        ``reply_context``, if non-empty, is prepended to ``prompt`` as a
        ``<<<REPLY_CONTEXT>>>``-fenced block so the model can ground "no,
        the second one" / "did that finish?" against a specific prior run.
        """
        status = "ok"
        async with self._chat_lock:
            runner: Optional[CursorRunner] = None
            try:
                runner = await self._do_persistent_chat_run(
                    prompt=prompt,
                    bubble=bubble,
                    reply_context=reply_context,
                )
            except asyncio.CancelledError:
                status = "interrupted"
                try:
                    await bubble.finalize("(interrupted)")
                finally:
                    await bubble.aclose()
                self._record_chat_finish(bubble, status, runner)
                raise
            except Exception as exc:
                status = "error"
                logger.exception("cursor-agent run crashed")
                try:
                    await bubble.finalize(f"cursor-agent crashed: {exc}")
                except Exception:
                    pass
            finally:
                await bubble.aclose()
                if status != "interrupted":
                    self._record_chat_finish(bubble, status, runner)

    def _record_chat_finish(
        self,
        bubble: Bubble,
        status: str,
        runner: Optional[CursorRunner],
    ) -> None:
        try:
            self._runs.record_finish(
                bubble.message_id,
                status=status,
                final_text=(runner.final_text if runner else "") or "",
                tool_log=(list(runner.tool_log) if runner else []),
                session_id=(runner.session_id if runner else None),
            )
        except Exception:
            logger.exception("runjournal record_finish (chat) crashed")

    async def _do_persistent_chat_run(
        self,
        *,
        prompt: str,
        bubble: Bubble,
        reply_context: str = "",
    ) -> CursorRunner:
        resume_id = self._chat_session_id
        # The REPLY_CONTEXT block is conceptually system-level
        # instructions for resolving a referent, but cursor-agent has no
        # dedicated channel for it; we tack it onto the user prompt with
        # explicit fences so the model treats it as authoritative.
        full_prompt = (
            (reply_context + "\n\n" + prompt) if reply_context else prompt
        )

        runner = CursorRunner(
            prompt=full_prompt,
            workdir=self.workdir,
            model=CHAT_MODEL,
            system_prompt=self._system_prompt(),
            extra_prompts=self._extra_prompts(),
            resume_session=resume_id,
        )
        async with self._make_supervisor(bubble, INITIAL_BUBBLE_TEXT) as sup:
            await runner.run(on_update=sup.on_update, on_final=sup.on_final)

        if runner.resume_failed and resume_id:
            logger.warning(
                "resume of chat %s failed; clearing and starting fresh",
                resume_id,
            )
            self._chat_session_id = None
            session_store.clear(self.paths)
            runner = CursorRunner(
                prompt=full_prompt,
                workdir=self.workdir,
                model=CHAT_MODEL,
                system_prompt=self._system_prompt(),
                extra_prompts=self._extra_prompts(),
            )
            async with self._make_supervisor(bubble, INITIAL_BUBBLE_TEXT) as sup:
                await runner.run(on_update=sup.on_update, on_final=sup.on_final)

        new_id = runner.session_id
        if new_id and new_id != self._chat_session_id:
            self._chat_session_id = new_id
            try:
                session_store.save(
                    self.paths,
                    ChatSession.now(session_id=new_id, model=CHAT_MODEL),
                )
                logger.info("persisted new chat session id: %s", new_id)
            except OSError as exc:
                logger.warning(
                    "could not persist chat session %s: %s", new_id, exc
                )
        return runner

    async def _transcribe_voice(
        self,
        http: httpx.AsyncClient,
        audio: dict[str, Any],
    ) -> str:
        """Download a Telegram voice file and transcribe via OpenAI Whisper.

        Raises ``RuntimeError`` with a short, user-facing message on any
        failure (missing key, Telegram error, download failure, Whisper
        non-200, or empty transcript).
        """
        if not self.openai_api_key:
            raise RuntimeError("OPENAI_API_KEY not configured")

        resp = await self._api(http, "getFile", file_id=audio["file_id"])
        if resp.status_code != 200:
            raise RuntimeError(f"getFile HTTP {resp.status_code}")
        body = resp.json()
        if not body.get("ok"):
            raise RuntimeError(f"getFile not ok: {body!r}")
        file_path = (body.get("result") or {}).get("file_path")
        if not file_path:
            raise RuntimeError("getFile returned no file_path")

        file_url = f"{BOT_API}/file/bot{self.bot_token}/{file_path}"
        try:
            download = await http.get(file_url, timeout=WHISPER_TIMEOUT)
        except httpx.HTTPError as exc:
            raise RuntimeError(f"audio download failed: {exc}") from exc
        if download.status_code != 200:
            raise RuntimeError(f"audio download HTTP {download.status_code}")
        audio_bytes = download.content
        if not audio_bytes:
            raise RuntimeError("downloaded audio is empty")

        filename = file_path.rsplit("/", 1)[-1] or "audio.ogg"
        mime = audio["mime_type"] or "audio/ogg"

        try:
            transcribe_resp = await http.post(
                OPENAI_TRANSCRIPTIONS_URL,
                headers={"Authorization": f"Bearer {self.openai_api_key}"},
                files={"file": (filename, audio_bytes, mime)},
                data={"model": WHISPER_MODEL, "response_format": "text"},
                timeout=WHISPER_TIMEOUT,
            )
        except httpx.HTTPError as exc:
            raise RuntimeError(f"whisper request failed: {exc}") from exc

        if transcribe_resp.status_code != 200:
            raise RuntimeError(
                f"whisper HTTP {transcribe_resp.status_code}: "
                f"{transcribe_resp.text[:200]}"
            )

        transcript = transcribe_resp.text.strip()
        if not transcript:
            raise RuntimeError("whisper returned empty transcript")
        return transcript

    async def _handle_reset_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        """Wipe the persistent chat session and ack with a bubble."""
        sent = await self._send_initial_bubble(http, message, text="resetting…")
        if sent is None:
            return
        self._journal_command_start(
            sent["message_id"],
            kind="reset",
            message=message,
            trigger_text="/reset",
        )
        bubble = self._make_bubble(http, sent["message_id"])
        try:
            await self._handle_reset_action(bubble)
        finally:
            await bubble.aclose()
            self._journal_command_finish(
                bubble.message_id,
                status="ok",
                final_text=bubble.last_sent or "",
            )

    async def _handle_reset_action(self, bubble: Bubble) -> None:
        """Reset behaviour, given an already-sent bubble to finalise into."""
        async with self._chat_lock:
            prev = self._chat_session_id
            removed = session_store.clear(self.paths)
            self._chat_session_id = None

        if prev:
            text = f"started a fresh chat (cleared session {prev})"
        elif removed:
            text = "started a fresh chat (cleared stale session file)"
        else:
            text = "no chat session to reset; next message will start one"
        await bubble.finalize(text)

    async def _handle_update_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        """Hard-reset the arbos checkout to ``origin/main``, optionally
        reinstall, then trigger a detached ``pm2 reload`` so we restart on
        the new code.

        Driven step-by-step from the event loop so the bubble can stream
        progress between phases. Each shell-out is itself sync but short
        (~hundreds of ms), so we briefly block the loop -- acceptable given
        ``/update`` is rare and intentionally exclusive.
        """
        sent = await self._send_initial_bubble(http, message, text="updating…")
        if sent is None:
            # _send_initial_bubble already logged the HTTP failure at WARNING.
            # Log here too so the /update lifecycle has a clean "started ->
            # bailed at bubble" trail rather than disappearing into silence.
            logger.warning("/update aborted: could not send initial bubble")
            return
        logger.info("/update bubble sent (msg_id=%s); starting update run", sent["message_id"])
        self._journal_command_start(
            sent["message_id"],
            kind="update",
            message=message,
            trigger_text="/update",
        )
        bubble = self._make_bubble(http, sent["message_id"])
        await self._handle_update_action(bubble)

    async def _handle_update_action(self, bubble: Bubble) -> None:
        """Update behaviour, given an already-sent bubble to drive."""
        status = "ok"
        try:
            await self._do_update_run(bubble)
        except asyncio.CancelledError:
            status = "interrupted"
            try:
                await bubble.finalize("(interrupted)")
            finally:
                await bubble.aclose()
            self._journal_command_finish(
                bubble.message_id, status=status, final_text=bubble.last_sent or ""
            )
            raise
        except Exception as exc:
            status = "error"
            logger.exception("/update crashed")
            try:
                await bubble.finalize(f"update crashed: {exc}")
            except Exception:
                pass
            await bubble.aclose()
        else:
            await bubble.aclose()
        if status != "interrupted":
            self._journal_command_finish(
                bubble.message_id, status=status, final_text=bubble.last_sent or ""
            )

    async def _do_update_run(self, bubble: Bubble) -> None:
        try:
            src = updater.resolve_src_dir(self.paths)
        except UpdateError as exc:
            logger.warning("/update: resolve_src_dir failed: %s", exc)
            await bubble.finalize(f"update failed: {exc}")
            return
        logger.info("/update: src=%s; fetching origin/%s", src, updater.UPDATE_BRANCH)

        await bubble.update(f"fetching origin/{updater.UPDATE_BRANCH} in {src}…")
        try:
            old_sha, old_subject, new_sha, new_subject, dirty = await asyncio.to_thread(
                updater.fetch_and_reset, src
            )
        except UpdateError as exc:
            logger.warning("/update: fetch_and_reset failed: %s", exc)
            await bubble.finalize(f"update failed: {exc}")
            return
        logger.info("/update: old=%s new=%s dirty=%d", old_sha, new_sha, len(dirty))

        reinstalled = False
        if old_sha != new_sha:
            try:
                changed = await asyncio.to_thread(
                    updater._changed_paths, src, old_sha, new_sha
                )
            except UpdateError as exc:
                logger.warning("/update: _changed_paths failed: %s", exc)
                await bubble.finalize(f"update failed (diff): {exc}")
                return
            if updater._needs_reinstall(changed):
                logger.info("/update: reinstall trigger files changed; running reinstall")
                await bubble.update(
                    f"old: {old_sha} -> new: {new_sha}\nreinstalling deps…"
                )
                try:
                    await asyncio.to_thread(updater._reinstall, src)
                except UpdateError as exc:
                    logger.warning("/update: reinstall failed: %s", exc)
                    await bubble.finalize(f"update failed (reinstall): {exc}")
                    return
                reinstalled = True
                logger.info("/update: reinstall OK")

        result = UpdateResult(
            src_dir=src,
            old_sha=old_sha,
            new_sha=new_sha,
            old_subject=old_subject,
            new_subject=new_subject,
            reinstalled=reinstalled,
            dirty_wiped=dirty,
        )

        if not result.changed:
            logger.info("/update: already up to date at %s", new_sha)
            await bubble.finalize(
                updater.render_summary(result, pm2_name=self.pm2_name, restarting=False)
            )
            return

        await bubble.finalize(
            updater.render_summary(result, pm2_name=self.pm2_name, restarting=True)
        )

        try:
            updater.spawn_pm2_reload(self.pm2_name)
            logger.info(
                "/update: spawned `pm2 reload %s`; waiting to be replaced",
                self.pm2_name,
            )
        except UpdateError as exc:
            logger.warning("pm2 reload failed: %s", exc)
            try:
                await bubble.update(
                    updater.render_summary(result, pm2_name=self.pm2_name, restarting=False)
                    + f"\n\npm2 reload failed: {exc}\nrestart manually with `./run.sh restart`"
                )
            except Exception:
                pass

    async def _handle_restart_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        """Trigger a clean ``pm2 reload arbos-<machine>``.

        UX is single-bubble: we send "restarting…", drop a marker file
        pointing at this bubble, and spawn a detached pm2 reload. The
        post-restart agent (see :meth:`_finalize_restart_marker`) then
        edits the same bubble in-place to "✅ back online — restart took
        Xs", so the whole event lives in one Telegram message.
        """
        sent = await self._send_initial_bubble(http, message, text="restarting…")
        if sent is None:
            logger.warning("/restart aborted: could not send initial bubble")
            return
        logger.info(
            "/restart bubble sent (msg_id=%s); starting restart run",
            sent["message_id"],
        )
        self._journal_command_start(
            sent["message_id"],
            kind="restart",
            message=message,
            trigger_text="/restart",
        )
        bubble = self._make_bubble(http, sent["message_id"])
        await self._handle_restart_action(bubble)

    async def _handle_restart_action(self, bubble: Bubble) -> None:
        """Restart behaviour, given an already-sent bubble to drive."""
        status = "ok"
        try:
            await self._do_restart_run(bubble)
        except asyncio.CancelledError:
            status = "interrupted"
            try:
                await bubble.finalize("(interrupted)")
            finally:
                await bubble.aclose()
            self._journal_command_finish(
                bubble.message_id, status=status, final_text=bubble.last_sent or ""
            )
            raise
        except Exception as exc:
            status = "error"
            logger.exception("/restart crashed")
            try:
                await bubble.finalize(f"restart crashed: {exc}")
            except Exception:
                pass
            await bubble.aclose()
        # NOTE: ok-path doesn't call _journal_command_finish here on
        # purpose -- /restart's bubble finalisation lives in the *next*
        # process via _finalize_restart_marker. We update the journal
        # there instead so the entry reflects the "back online" text the
        # user actually saw.
        if status not in ("interrupted", "ok"):
            self._journal_command_finish(
                bubble.message_id, status=status, final_text=bubble.last_sent or ""
            )

    async def _do_restart_run(self, bubble: Bubble) -> None:
        # Persist enough info for the post-restart agent to re-attach to
        # this exact bubble. Written *before* spawning pm2 reload so even
        # if pm2 SIGINTs us between the spawn and our own bubble.aclose,
        # the new agent still finds the marker.
        marker = {
            "version": 1,
            "chat_id": self.chat_id,
            "thread_id": self.topic_id,
            "message_id": bubble.message_id,
            "machine": self.machine,
            "started_at": time.time(),
            "pid": os.getpid(),
        }
        try:
            self._write_restart_marker(marker)
        except OSError as exc:
            logger.warning("/restart: could not write marker: %s", exc)
            await bubble.finalize(
                f"restart failed: could not write marker ({exc})"
            )
            return

        await bubble.update(
            f"restarting {self.pm2_name}…\n(pm2 reload incoming)"
        )

        try:
            updater.spawn_pm2_reload(self.pm2_name)
            logger.info(
                "/restart: spawned `pm2 reload %s`; waiting to be replaced",
                self.pm2_name,
            )
        except UpdateError as exc:
            logger.warning("/restart: pm2 reload failed: %s", exc)
            self._delete_restart_marker()
            try:
                await bubble.finalize(
                    f"restart failed: {exc}\n"
                    "fix manually with `./run.sh restart`"
                )
            except Exception:
                pass
            return

        # Don't finalize here -- the *new* agent will edit this same
        # bubble to "back online" via _finalize_restart_marker(). Just
        # flush the pending "restarting…" snapshot so the user actually
        # sees something between SIGTERM and the new boot.
        await bubble.update(
            f"restarting {self.pm2_name}…\n"
            f"⏳ waiting for the new process to come up"
        )
        # Give the flusher a short window to push the latest snapshot
        # before pm2 SIGINTs us. The flusher is rate-limited to one edit
        # per 2.5s, so anything shorter risks losing the update.
        try:
            await asyncio.sleep(3.0)
        except asyncio.CancelledError:
            pass
        # Best-effort close; pm2 may already be SIGTERMing us.
        try:
            await bubble.aclose()
        except Exception:
            pass

    def _write_restart_marker(self, marker: dict[str, Any]) -> None:
        """Atomically persist the restart marker."""
        path = self.paths.restart_marker_path
        path.parent.mkdir(parents=True, exist_ok=True)
        tmp = path.with_suffix(path.suffix + ".tmp")
        tmp.write_text(json.dumps(marker, sort_keys=True))
        os.replace(tmp, path)

    def _delete_restart_marker(self) -> None:
        try:
            self.paths.restart_marker_path.unlink()
        except FileNotFoundError:
            pass
        except OSError as exc:
            logger.warning("could not delete restart marker: %s", exc)

    async def _finalize_restart_marker(self, http: httpx.AsyncClient) -> None:
        """If a previous run dropped a restart marker, edit that bubble to
        announce we're back online, then delete the marker."""
        path = self.paths.restart_marker_path
        try:
            raw = path.read_text()
        except FileNotFoundError:
            return
        except OSError as exc:
            logger.warning("could not read restart marker: %s", exc)
            return

        try:
            marker = json.loads(raw)
        except json.JSONDecodeError as exc:
            logger.warning("restart marker malformed (%s); deleting", exc)
            self._delete_restart_marker()
            return

        chat_id = marker.get("chat_id")
        thread_id = marker.get("thread_id")
        message_id = marker.get("message_id")
        started_at = marker.get("started_at")
        if not (
            isinstance(chat_id, int)
            and isinstance(thread_id, int)
            and isinstance(message_id, int)
        ):
            logger.warning("restart marker missing fields; deleting: %r", marker)
            self._delete_restart_marker()
            return

        # Only honour the marker if it's for *this* topic; otherwise it
        # belongs to a different machine sharing the .arbos dir layout
        # (shouldn't happen, but defensive).
        if chat_id != self.chat_id or thread_id != self.topic_id:
            logger.warning(
                "restart marker for chat=%s thread=%s; we are chat=%s thread=%s; ignoring",
                chat_id, thread_id, self.chat_id, self.topic_id,
            )
            self._delete_restart_marker()
            return

        elapsed = ""
        if isinstance(started_at, (int, float)):
            dt = max(0.0, time.time() - float(started_at))
            elapsed = f" — restart took {dt:.1f}s"

        bubble = self._make_bubble(
            http, message_id, chat_id=chat_id, message_thread_id=thread_id
        )
        try:
            await bubble.finalize(
                f"✅ {self.pm2_name} back online{elapsed}"
            )
            logger.info(
                "/restart: finalized bubble msg_id=%s after %s",
                message_id, elapsed.strip(" —") or "?",
            )
            # Close out the restart bubble's journal entry (started in
            # the previous process before the pm2 reload) with the
            # "back online" text the user actually saw.
            self._journal_command_finish(
                message_id,
                status="ok",
                final_text=bubble.last_sent or "",
            )
        except Exception:
            logger.exception("/restart: failed to finalize back-online bubble")
        finally:
            await bubble.aclose()
            self._delete_restart_marker()

    async def _handle_plan_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        request: str,
        *,
        reply_context: str = "",
    ) -> None:
        if not request:
            sent = await self._send_initial_bubble(
                http, message, text="usage: /plan <what you want done>"
            )
            if sent is not None:
                bubble = self._make_bubble(http, sent["message_id"])
                await bubble.aclose()
            return

        sender_label = self._trigger_sender_label(message)
        trigger_msg_id = message.get("message_id")

        # --- Phase 1: plan ------------------------------------------------
        sent1 = await self._send_initial_bubble(
            http, message, text=PLAN_INITIAL_BUBBLE_TEXT
        )
        if sent1 is None:
            return

        bubble1 = self._make_bubble(http, sent1["message_id"])

        # Journal the plan bubble immediately so a reply hitting it mid-run
        # already gets a "(in progress)" context block.
        self._runs.record_start(
            bubble1.message_id,
            kind="plan",
            trigger_message_id=trigger_msg_id,
            trigger_text=f"/plan {request}",
            trigger_sender=sender_label,
            model=PLAN_MODEL,
        )

        plan_prompt = PLAN_PROMPT_PREAMBLE + request
        if reply_context:
            plan_prompt = reply_context + "\n\n" + plan_prompt

        plan_runner = CursorRunner(
            prompt=plan_prompt,
            workdir=self.workdir,
            model=PLAN_MODEL,
            plan_mode=True,
            system_prompt=self._system_prompt(),
            extra_prompts=self._extra_prompts(),
        )
        plan_holder: dict[str, str] = {"text": "", "error": ""}
        plan_status = "ok"

        try:
            async with self._make_supervisor(bubble1, PLAN_INITIAL_BUBBLE_TEXT) as sup:
                async def _final_with_capture(text: str) -> None:
                    # Capture the plan text for phase 2 *before* delegating
                    # to the supervisor (which will finalize the bubble).
                    plan_holder["text"] = text
                    await sup.on_final(text)
                await plan_runner.run(
                    on_update=sup.on_update,
                    on_final=_final_with_capture,
                )
        except asyncio.CancelledError:
            plan_status = "interrupted"
            try:
                await bubble1.finalize("(interrupted)")
            finally:
                await bubble1.aclose()
            try:
                self._runs.record_finish(
                    bubble1.message_id,
                    status=plan_status,
                    final_text="(interrupted)",
                    tool_log=list(plan_runner.tool_log),
                    session_id=plan_runner.session_id,
                )
            except Exception:
                logger.exception("runjournal record_finish (plan) crashed")
            raise
        except Exception as exc:
            plan_status = "error"
            logger.exception("cursor-agent plan-mode run crashed")
            plan_holder["error"] = str(exc)
            try:
                await bubble1.finalize(f"cursor-agent crashed: {exc}")
            except Exception:
                pass
        finally:
            await bubble1.aclose()
            if plan_status != "interrupted":
                try:
                    self._runs.record_finish(
                        bubble1.message_id,
                        status=plan_status,
                        final_text=(
                            plan_runner.final_text
                            or plan_holder["text"]
                            or ""
                        ),
                        tool_log=list(plan_runner.tool_log),
                        session_id=plan_runner.session_id,
                    )
                except Exception:
                    logger.exception("runjournal record_finish (plan) crashed")

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

        bubble2 = self._make_bubble(http, sent2["message_id"])

        # Cross-link the impl bubble back to the plan bubble so a reply
        # to either surfaces the other half via `paired_bubble_id`.
        self._runs.record_start(
            bubble2.message_id,
            kind="impl",
            trigger_message_id=trigger_msg_id,
            trigger_text=f"/plan {request}",
            trigger_sender=sender_label,
            model=IMPL_MODEL,
            paired_bubble_id=bubble1.message_id,
        )

        impl_prompt = (
            IMPL_PROMPT_PREAMBLE
            + f"Original request:\n{request}\n\nPlan:\n{plan_text}"
        )
        impl_runner = CursorRunner(
            prompt=impl_prompt,
            workdir=self.workdir,
            model=IMPL_MODEL,
            system_prompt=self._system_prompt(),
            extra_prompts=self._extra_prompts(),
        )
        impl_status = "ok"
        try:
            async with self._make_supervisor(bubble2, IMPL_INITIAL_BUBBLE_TEXT) as sup:
                await impl_runner.run(on_update=sup.on_update, on_final=sup.on_final)
        except asyncio.CancelledError:
            impl_status = "interrupted"
            try:
                await bubble2.finalize("(interrupted)")
            finally:
                await bubble2.aclose()
            try:
                self._runs.record_finish(
                    bubble2.message_id,
                    status=impl_status,
                    final_text="(interrupted)",
                    tool_log=list(impl_runner.tool_log),
                    session_id=impl_runner.session_id,
                )
            except Exception:
                logger.exception("runjournal record_finish (impl) crashed")
            raise
        except Exception as exc:
            impl_status = "error"
            logger.exception("cursor-agent implementation run crashed")
            try:
                await bubble2.finalize(f"cursor-agent crashed: {exc}")
            except Exception:
                pass
        finally:
            await bubble2.aclose()
            if impl_status != "interrupted":
                try:
                    self._runs.record_finish(
                        bubble2.message_id,
                        status=impl_status,
                        final_text=impl_runner.final_text or "",
                        tool_log=list(impl_runner.tool_log),
                        session_id=impl_runner.session_id,
                    )
                except Exception:
                    logger.exception("runjournal record_finish (impl) crashed")

        # Once the impl half lands, back-link the plan record to it so a
        # reply to the plan bubble can surface the impl outcome too.
        try:
            plan_rec = self._runs.get(bubble1.message_id)
            if plan_rec is not None and plan_rec.paired_bubble_id is None:
                plan_rec.paired_bubble_id = bubble2.message_id
                self._runs._atomic_write(bubble1.message_id, plan_rec)
        except Exception:
            logger.exception("runjournal plan back-link crashed")

    # ---- confirmation flow ----------------------------------------------
    async def _prompt_confirmation(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        *,
        label: str,
        description: str,
        action: Callable[[Bubble], Awaitable[None]],
    ) -> None:
        """Post a Confirm/Cancel bubble that, on Confirm, runs ``action(bubble)``.

        ``label`` is a short tag for logs and the bubble headline.
        ``description`` is a one-liner shown to the user above the buttons.
        Only the original sender (Telegram ``from.id``) is allowed to click;
        others get a toast and the bubble stays armed.
        """
        text = f"confirm {label}?\n{description}"
        markup = {
            "inline_keyboard": [[
                {"text": "✅ Confirm", "callback_data": f"confirm:{label}"},
                {"text": "✖️ Cancel", "callback_data": f"cancel:{label}"},
            ]]
        }
        sent = await self._send_initial_bubble(
            http, message, text=text, reply_markup=markup
        )
        if sent is None:
            logger.warning("confirmation bubble for %s could not be sent", label)
            return

        # Opportunistic GC of expired entries so the dict can't grow without
        # bound on a long-lived process.
        self._gc_pending_confirms()

        self._journal_command_start(
            sent["message_id"],
            kind="confirm",
            message=message,
            trigger_text=f"/confirm {label}",
        )

        sender = message.get("from") or {}
        self._pending_confirms[sent["message_id"]] = _PendingConfirm(
            bubble_message_id=sent["message_id"],
            user_id=sender.get("id"),
            label=label,
            original_message=message,
            action=action,
        )

    def _gc_pending_confirms(self) -> None:
        now = time.monotonic()
        stale = [
            mid for mid, entry in self._pending_confirms.items()
            if entry.expires_at < now
        ]
        for mid in stale:
            self._pending_confirms.pop(mid, None)
        if stale:
            logger.info("dropped %d expired confirmation(s)", len(stale))

    async def _handle_confirm_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        wrapped: str,
    ) -> None:
        """Gate any prompt or sub-command behind a Confirm/Cancel bubble.

        If ``wrapped`` is itself ``/update`` or ``/reset`` we route to the
        existing destructive-op handlers; otherwise the wrapped text is
        treated as a fresh prompt for the persistent chat.
        """
        if not wrapped:
            sent = await self._send_initial_bubble(
                http, message, text="usage: /confirm <command-or-prompt>"
            )
            if sent is not None:
                bubble = self._make_bubble(http, sent["message_id"])
                await bubble.aclose()
            return

        if self._match_update_command(wrapped):
            label = "/update"
            description = (
                "git fetch + hard-reset to origin/main, reinstall if "
                "needed, then pm2 reload this machine"
            )
            action: Callable[[Bubble], Awaitable[None]] = lambda b: self._handle_update_action(b)
        elif self._match_restart_command(wrapped):
            label = "/restart"
            description = "pm2 reload this machine (no code changes)"
            action = lambda b: self._handle_restart_action(b)
        elif self._match_reset_command(wrapped):
            label = "/reset"
            description = "wipe the persistent chat session"
            action = lambda b: self._handle_reset_action(b)
        else:
            preview = wrapped if len(wrapped) <= 80 else wrapped[:77] + "…"
            label = "prompt"
            description = f"run: {preview}"
            wrapped_prompt = wrapped
            action = lambda b, _p=wrapped_prompt: self._run_persistent_chat(
                prompt=_p, bubble=b
            )

        await self._prompt_confirmation(
            http,
            message,
            label=label,
            description=description,
            action=action,
        )

    async def _handle_callback_query(
        self,
        http: httpx.AsyncClient,
        cb: dict[str, Any],
    ) -> None:
        """React to a Confirm/Cancel button press on a bubble we own.

        Acks the callback (so the spinner clears), authenticates the
        clicker against the original sender, removes the inline keyboard,
        and either runs the captured action or finalises the bubble with
        "cancelled".
        """
        cb_id = cb.get("id")
        data = cb.get("data") or ""
        msg = cb.get("message") or {}
        bubble_msg_id = msg.get("message_id")
        clicker = (cb.get("from") or {}).get("id")

        async def _answer(text: str = "", *, alert: bool = False) -> None:
            if not cb_id:
                return
            params: dict[str, Any] = {"callback_query_id": cb_id}
            if text:
                params["text"] = text
                params["show_alert"] = alert
            try:
                await self._api(http, "answerCallbackQuery", **params)
            except Exception:
                logger.exception("answerCallbackQuery failed")

        if not bubble_msg_id or not data:
            await _answer()
            return

        # Only react to callbacks on bubbles in our own topic; ignore others.
        if (msg.get("chat") or {}).get("id") != self.chat_id:
            await _answer()
            return
        if msg.get("message_thread_id") != self.topic_id:
            await _answer()
            return

        action_kind, _, _ = data.partition(":")
        entry = self._pending_confirms.get(bubble_msg_id)
        if entry is None:
            await _answer("expired or already handled", alert=False)
            return

        if entry.user_id is not None and clicker is not None and clicker != entry.user_id:
            await _answer("not your prompt", alert=False)
            return

        # Take ownership of the entry so a double-click can't run twice.
        self._pending_confirms.pop(bubble_msg_id, None)

        bubble = self._make_bubble(http, bubble_msg_id)

        try:
            await bubble.clear_reply_markup()
        except Exception:
            logger.exception("clear_reply_markup failed; continuing")

        if action_kind != "confirm":
            await _answer("cancelled")
            try:
                await bubble.finalize(f"{entry.label}: cancelled")
            finally:
                await bubble.aclose()
            self._journal_command_finish(
                bubble_msg_id,
                status="ok",
                final_text=bubble.last_sent or f"{entry.label}: cancelled",
            )
            return

        await _answer("confirmed")
        logger.info(
            "callback confirmed: label=%s bubble=%s clicker=%s",
            entry.label, bubble_msg_id, clicker,
        )
        try:
            await entry.action(bubble)
        except asyncio.CancelledError:
            try:
                await bubble.finalize("(interrupted)")
            finally:
                await bubble.aclose()
            self._journal_command_finish(
                bubble_msg_id,
                status="interrupted",
                final_text=bubble.last_sent or "(interrupted)",
            )
            raise
        except Exception as exc:
            logger.exception("confirmed action %s crashed", entry.label)
            try:
                await bubble.finalize(f"{entry.label} crashed: {exc}")
            except Exception:
                pass
            await bubble.aclose()
            self._journal_command_finish(
                bubble_msg_id,
                status="error",
                final_text=bubble.last_sent or f"{entry.label} crashed: {exc}",
            )

    def _spawn_callback_handler(
        self,
        http: httpx.AsyncClient,
        cb: dict[str, Any],
    ) -> None:
        """Run :meth:`_handle_callback_query` as a tracked task.

        Callbacks are intentionally NOT journaled in the inflight store --
        they're transient UI. If the agent dies mid-callback, the user
        re-clicks (or re-issues if the bubble's TTL lapsed across restart).
        """
        cb_id = cb.get("id")
        task = asyncio.create_task(
            self._handle_callback_query(http, cb),
            name=f"handle-cb-{cb_id}",
        )
        self._tasks.add(task)

        def _on_done(t: asyncio.Task[None]) -> None:
            self._tasks.discard(t)
            if t.cancelled():
                return
            exc = t.exception()
            if exc is not None:
                logger.error(
                    "callback handler crashed (cb_id=%s): %s",
                    cb_id, exc, exc_info=exc,
                )

        task.add_done_callback(_on_done)

    def _spawn_handler(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        *,
        update_id: int,
    ) -> None:
        msg_id = message.get("message_id")
        sender = (message.get("from") or {}).get("username") or (
            message.get("from") or {}
        ).get("id")
        task = asyncio.create_task(
            self._handle_message(http, message),
            name=f"handle-msg-{msg_id}",
        )
        self._tasks.add(task)

        def _on_done(t: asyncio.Task[None]) -> None:
            self._tasks.discard(t)
            if t.cancelled():
                # We only cancel tasks during graceful shutdown, where
                # mark_interrupted_all() has already touched the journal.
                # Don't override that here.
                return
            exc = t.exception()
            if exc is not None:
                # Without this hook, asyncio loses unhandled task exceptions
                # to "Task exception was never retrieved" at GC time and they
                # never reach our log -- which is exactly how /update on
                # templar (07:42-08:54Z) failed silently. Log them here so
                # every handler crash leaves a breadcrumb in agent.log.
                logger.error(
                    "message handler crashed (msg_id=%s sender=%s): %s",
                    msg_id, sender, exc, exc_info=exc,
                )
                try:
                    self._inflight.mark_failed(update_id, f"{type(exc).__name__}: {exc}")
                except Exception:
                    logger.exception("inflight mark_failed crashed")
                return
            try:
                self._inflight.mark_done(update_id)
            except Exception:
                logger.exception("inflight mark_done crashed")

        task.add_done_callback(_on_done)

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

            outbox = OutboxWatcher(
                http=http,
                bot_token=self.bot_token,
                chat_id=self.chat_id,
                message_thread_id=self.topic_id,
                paths=self.paths,
            )
            outbox.start()

            try:
                # If the previous process spawned `pm2 reload` for a
                # `/restart`, finalize that bubble in-place so the user
                # sees a clean "back online" message in the same chat
                # bubble they triggered the restart from. Runs before
                # inflight recovery so the back-online message lands
                # before any "I was restarted" notes from interrupted
                # handlers.
                await self._finalize_restart_marker(http)
                # Replay anything we owed the user from the previous run
                # before we accept new updates. This deliberately runs
                # *after* the outbox watcher is up so any media the
                # replayed handlers produce can flow back out.
                await self._recover_inflight(http)
                await self._poll_loop(http, offset)
            finally:
                await outbox.stop()
                await self._drain_tasks()
                # Any handler tasks still pending at this point either
                # raced the cancellation or refused it; either way we
                # mark them ``interrupted`` so the next boot can replay
                # them with correct provenance.
                try:
                    self._inflight.mark_interrupted_all()
                except Exception:
                    logger.exception("inflight mark_interrupted_all crashed")
                # Same idea for the per-bubble run journal: any entry
                # still flagged ``running`` is a bubble whose finalize
                # got cut off by SIGINT/SIGTERM before the agent's own
                # finally-clause could write record_finish. Flip them
                # so a future reply doesn't show stale "(in progress)".
                try:
                    self._runs.mark_running_as_interrupted()
                except Exception:
                    logger.exception("runjournal sweep crashed")

        logger.info("cursor-agent stopping (signal received)")

    async def _poll_loop(self, http: httpx.AsyncClient, offset: int) -> None:
        while not self._stop.is_set():
            try:
                resp = await self._api(
                    http,
                    "getUpdates",
                    offset=offset,
                    timeout=LONG_POLL_TIMEOUT,
                    allowed_updates=["message", "callback_query"],
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
                update_id = upd["update_id"]
                offset = update_id + 1
                msg = upd.get("message")
                cb = upd.get("callback_query")
                if msg and self._is_for_us(msg):
                    # Journal first (so a crash between here and the next
                    # line cannot leave us with an advanced offset and no
                    # record of what we owed the user), then advance the
                    # Telegram offset, then spawn the actual handler.
                    try:
                        self._inflight.mark_pending(update_id, msg)
                    except Exception:
                        # If the journal is unwritable we still take the
                        # message -- losing crash-recovery is bad, but
                        # silently rejecting live messages is worse.
                        logger.exception(
                            "inflight mark_pending failed; proceeding without journal"
                        )
                    self._save_offset(offset)
                    self._spawn_handler(http, msg, update_id=update_id)
                elif cb is not None:
                    # Callback queries are transient UI -- never journaled
                    # (see _spawn_callback_handler). Advance the offset
                    # immediately so a crash mid-callback doesn't loop.
                    self._save_offset(offset)
                    self._spawn_callback_handler(http, cb)
                else:
                    self._save_offset(offset)

    async def _recover_inflight(self, http: httpx.AsyncClient) -> None:
        """Replay or apologise for handlers that didn't finish last run.

        Called once at startup, *before* the main poll loop. For each
        ``pending`` / ``interrupted`` entry in the journal:

        * If the message is a plain text dispatch (or a voice/video note
          with no text), replay it through the normal handler path. The
          user's original message stays in the chat; we just produce a
          fresh bubble underneath. Replays use the same ``update_id`` so
          their journal row gets updated in-place rather than orphaned.
        * If the message is a ``/`` command we treat as too-side-effecty
          to safely re-run (``/update``, ``/reset``, ``/new``, ``/plan``),
          we don't re-execute -- we just post a short note to the topic
          asking the user to re-issue it, and mark the entry done.

        The decision is intentionally conservative: false positives (a
        plain message that wasn't actually idempotent) cost the user one
        duplicated answer; false negatives on a side-effecty replay cost
        the user a corrupted machine.
        """
        try:
            pending = self._inflight.pending()
        except Exception:
            logger.exception("inflight pending() crashed")
            return
        if not pending:
            return

        logger.info(
            "recovering %d in-flight handler(s) from previous run", len(pending)
        )
        for entry in pending:
            try:
                await self._recover_one(http, entry)
            except Exception:
                logger.exception(
                    "recovery for update_id=%s crashed; marking failed",
                    entry.update_id,
                )
                try:
                    self._inflight.mark_failed(
                        entry.update_id, "recovery raised; see agent.log"
                    )
                except Exception:
                    pass

    async def _recover_one(
        self,
        http: httpx.AsyncClient,
        entry: InflightEntry,
    ) -> None:
        text = entry.text
        message = entry.message
        if not message:
            self._inflight.mark_failed(entry.update_id, "no message in journal")
            return

        # Side-effecty commands: don't re-run, just tell the user.
        # /confirm is included because the in-memory pending entry it
        # created is gone after a restart -- replaying would just orphan
        # another bubble.
        is_command = (
            self._match_plan_command(text) is not None
            or self._match_reset_command(text)
            or self._match_update_command(text)
            or self._match_restart_command(text)
            or self._match_confirm_command(text) is not None
        )
        if is_command:
            head = text.partition(" ")[0] or "(command)"
            note = (
                f"⚠️ I was restarted while running `{head}` — that command "
                "isn't safe to auto-replay. Re-send it if you still want it."
            )
            try:
                await self._send_initial_bubble(http, message, text=note)
            except Exception:
                logger.exception("recovery: failed to post apology bubble")
            self._inflight.mark_done(entry.update_id)
            return

        # Plain text / voice path: replay through the normal handler.
        # _spawn_handler will overwrite the journal row in-place because
        # it's keyed by update_id, so we go from interrupted -> pending
        # -> done (or failed) without leaking entries.
        try:
            self._inflight.mark_pending(entry.update_id, message)
        except Exception:
            logger.exception("recovery: re-marking pending failed")
        logger.info(
            "replaying interrupted handler: update_id=%s text=%r",
            entry.update_id, text[:60],
        )
        self._spawn_handler(http, message, update_id=entry.update_id)

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
