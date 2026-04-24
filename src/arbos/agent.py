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
from .crons import CronEntry, CronStore
from .cursor_runner import CursorRunner
from . import inflight
from .inflight import InflightEntry, InflightStore
from .outbox import OutboxWatcher
from .prompts import build_prompt, load_extra_prompts
from .router import (
    DELEGATE_SLASH,
    RouterContext,
    RouterDecision,
    RouterOutcome,
    SmartRouter,
)
from . import runjournal
from .runjournal import RunJournal
from . import session_store
from .session_store import ChatSession
from .supervisor import Supervisor
from . import topic_state
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
# Placeholder shown in the routing bubble before the SmartRouter's
# supervisor flips it to "0:00 Routing"; visible only for the brief
# window between sendMessage and the supervisor's first push.
ROUTING_BUBBLE_TEXT = "routing…"

# Display name for the home (machine-default) topic's ``<name>`` slot.
# Renders as ``root · <machine>@<workspace>`` in the Telegram topic
# title so the machine's primary thread is visually distinct from any
# user-named topic adopted later.
HOME_TOPIC_NAME = "root"

OPENAI_TRANSCRIPTIONS_URL = "https://api.openai.com/v1/audio/transcriptions"
WHISPER_MODEL = "whisper-1"
WHISPER_TIMEOUT = 60.0

PLAN_MODEL = "claude-opus-4-7-high"
IMPL_MODEL = "claude-opus-4-7-high"
CHAT_MODEL = "claude-opus-4-7-high"

# SmartRouter runs in agent mode for EVERY user message before the heavy
# worker is even considered, so latency here directly compounds onto every
# reply. The job is small (read at most a couple of files / run a short
# shell, then either answer in 1-3 lines or emit one slash command), so a
# fast cheap model wins overall: routing finishes in a few seconds instead
# of ~15-25s with Opus, and the heavy Opus worker still owns anything the
# router delegates.
#
# Default is ``gpt-5.4-mini-medium`` -- small, fast, and well-behaved on
# structured output (the slash-command grammar). Override with
# ``ARBOS_ROUTER_MODEL`` to swap; the model slug must be one cursor-agent
# accepts (``cursor-agent --help`` lists them). The previous default
# ``claude-haiku-4-5`` was hallucinated and not actually a real cursor
# slug -- if cursor-agent rejects whatever ``ARBOS_ROUTER_MODEL`` is set
# to, the router will exit 1 and the agent will fall through to the
# worker (see ``_run_smart_router``'s ``decision is None`` branch).
ROUTER_MODEL = (
    os.environ.get("ARBOS_ROUTER_MODEL", "gpt-5.4-mini-medium").strip()
    or "gpt-5.4-mini-medium"
)

# Supervisor layer: a small LLM throttles the worker's noisy snapshot stream
# down to a single short status line every few seconds, so the bubble stays
# glanceable mid-run. Set ARBOS_SUPERVISOR_DISABLE=1 (or omit OPENAI_API_KEY)
# to bypass and stream raw snapshots straight to the bubble (legacy behavior).
SUPERVISOR_MODEL = (
    os.environ.get("ARBOS_SUPERVISOR_MODEL", "gpt-4.1-mini").strip()
    or "gpt-4.1-mini"
)
SUPERVISOR_INTERVAL = 2.0
# Per-rollout-line display budget. Keep this tight -- the rollout is a
# scannable timeline of short status lines, not a place for raw shell
# commands or long prose. The LLM live-summary is also told to stay
# within this budget via {max_chars} interpolation.
SUPERVISOR_MAX_CHARS = 80
# Per-bubble editMessageText floor for rollout bubbles. The default
# in bubble.py is 2.5s, picked to stay under Telegram's per-chat
# throttling ceiling. We push it down to 2.0s for the rollout bubble
# so the supervisor's live M:SS prefix advances every two seconds in
# real time (matches SUPERVISOR_INTERVAL). 429s, if any, are absorbed
# by the bubble's process-wide global backoff.
ROLLOUT_EDIT_INTERVAL = 2.0
SUPERVISOR_FINAL_MAX_CHARS = 1500
SUPERVISOR_STALL_HINT_AFTER = 15.0
SUPERVISOR_STALL_WARN_AFTER = 60.0
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
        # This is the *home* topic for this machine. The agent additionally
        # listens on any other topic in the same supergroup that has been
        # adopted via @-mention -- see ``_is_for_us`` and ``topic_state``.
        self.home_topic_id = _to_bot_thread_id(self.config.machine.topic_id)
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

        # ---- multi-topic state ------------------------------------------
        # Auto-adopt the home topic and load any other topics adopted on
        # previous boots. ``_adopted`` is the source of truth for "do we
        # answer messages in this thread"; it is mutated in-place when a
        # new topic is adopted via @-mention at runtime.
        try:
            # Home topic always renders as ``root · <machine>@<workspace>``:
            # the topic-creator's name component is fixed at "root" so the
            # machine's "main" thread is visually distinct from any other
            # topic the user adopts later. ``base_title`` is the machine
            # name (the "host" half of the ``<base>@<workspace>`` pair).
            topic_state.adopt(
                self.paths,
                self.home_topic_id,
                name=HOME_TOPIC_NAME,
                base_title=self.machine,
            )
        except Exception:
            logger.exception("could not adopt home topic %s", self.home_topic_id)
        # ---- one-shot migrations for legacy state ----------------------
        # 1. base_title backfill: every topic *this* agent answers in is
        #    owned by *this* machine, so any state file written before
        #    base_title existed is unambiguously self.machine.
        # 2. home-topic name migration: older boots auto-set
        #    name=self.machine on the home topic; rewrite to "root" so
        #    fresh installs and upgrades both render identically. Only
        #    rewrites the legacy default -- a user-chosen name (anything
        #    other than self.machine) is preserved.
        for tid in topic_state.list_adopted(self.paths):
            existing = topic_state.load(self.paths, tid)
            if existing is None:
                continue
            if not existing.base_title:
                try:
                    topic_state.adopt(
                        self.paths, tid, base_title=self.machine
                    )
                except Exception:
                    logger.exception(
                        "base_title backfill failed (topic=%s)", tid
                    )
            if (
                tid == self.home_topic_id
                and existing.name == self.machine
                and existing.name != HOME_TOPIC_NAME
            ):
                try:
                    migrated = topic_state.set_name(
                        self.paths, tid, HOME_TOPIC_NAME
                    )
                    if migrated is not None:
                        logger.info(
                            "home-topic name migrated: %r -> %r",
                            self.machine, HOME_TOPIC_NAME,
                        )
                except Exception:
                    logger.exception(
                        "home-topic name migration failed (topic=%s)", tid
                    )
        self._adopted: set[int] = set(topic_state.list_adopted(self.paths))
        self._adopted.add(self.home_topic_id)
        # Per-topic persistent cursor-agent chat sessions and the per-topic
        # serialisation lock that gates `--resume <id>` calls (two parallel
        # appends would interleave). Both are lazy: a topic only enters the
        # dicts once it has been touched.
        self._chat_sessions: dict[int, Optional[str]] = {}
        self._chat_locks: dict[int, asyncio.Lock] = {}
        # Per-topic SmartRouter sessions. Distinct from the worker's
        # chat session so routing-decision history and worker-coding
        # history never pollute each other; both threads see the same
        # incoming user messages but only one sees each side's replies.
        self._router_sessions: dict[int, Optional[str]] = {}
        self._router_locks: dict[int, asyncio.Lock] = {}
        for tid in list(self._adopted):
            loaded = session_store.load(self.paths, tid)
            if loaded is not None:
                self._chat_sessions[tid] = loaded.session_id
                logger.info(
                    "loaded persistent chat session for topic %s: %s",
                    tid, loaded.session_id,
                )
            loaded_router = session_store.load_router(self.paths, tid)
            if loaded_router is not None:
                self._router_sessions[tid] = loaded_router.session_id
                logger.info(
                    "loaded persistent router session for topic %s: %s",
                    tid, loaded_router.session_id,
                )
        # One outbox watcher per adopted topic; created in ``run()`` once
        # the agent-lifetime httpx client is open.
        self._outbox_watchers: dict[int, OutboxWatcher] = {}
        self._outbox_http: Optional[httpx.AsyncClient] = None
        # In-memory ``topic_id -> name`` cache populated from any
        # ``forum_topic_created`` / ``forum_topic_edited`` service
        # message we observe, even before the topic is adopted. Lets
        # ``_is_for_us`` pick up the user-given name on first @-mention
        # (the create event is a *separate* Telegram update that arrives
        # before the mention, so without this cache the name would be
        # discarded by the not-yet-adopted guard).
        self._topic_name_cache: dict[int, str] = {}

        # Persistent recurring-prompt schedules. Loaded from disk so cron
        # entries survive PM2 restarts and ``/update``. The agent spawns
        # one asyncio task per entry on boot (see ``_restore_crons``).
        self._crons = CronStore(self.paths)
        self._cron_tasks: dict[str, asyncio.Task[None]] = {}
        # Wall-clock for the *next* scheduled fire of each cron, set by
        # the loop right before it sleeps. Used by ``/cron list`` to
        # render "next in Ymss". Missing entry == not scheduled yet.
        self._cron_next_at: dict[str, float] = {}
        # Captured in ``run()`` from the agent-lifetime ``httpx.AsyncClient``
        # so cron loops have something to send Telegram traffic over.
        self._cron_http: Optional[httpx.AsyncClient] = None

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

        # Render the system preamble eagerly for the home topic so PROMPT.md
        # exists from t=0 even before the first message; build_prompt
        # persists to disk and caches per-(workspace, topic) for ~60s.
        try:
            build_prompt(
                self.paths,
                self.config,
                workspace=self._workdir_for(self.home_topic_id),
                topic_id=self.home_topic_id,
            )
        except Exception:
            logger.exception("initial PROMPT.md render failed; continuing")

        # Front-line routing agent. Each non-slash message spawns a
        # ``cursor-agent`` invocation in agent mode against this topic's
        # SmartRouter ``--resume`` session, so the routing decision sees
        # the topic's prior chat history. The agent ends its reply with
        # one of a small set of slash commands (/cron, /restart, ...,
        # /delegate) which we parse + dispatch via the existing handlers.
        # Falls open to delegate when ``ARBOS_SMART_ROUTER_DISABLE=1``
        # is set, so the agent remains fully functional even if the
        # router cursor-agent itself misbehaves.
        smart_router_disabled = (
            os.environ.get("ARBOS_SMART_ROUTER_DISABLE", "").strip()
            in ("1", "true", "True", "yes", "YES")
        )
        self._router = SmartRouter(enabled=not smart_router_disabled)
        if smart_router_disabled:
            logger.info(
                "ARBOS_SMART_ROUTER_DISABLE set; SmartRouter bypassed, "
                "all messages will delegate straight to the worker"
            )

    def _system_prompt(self, topic_id: int) -> str:
        """Cached system preamble for ``topic_id`` (refreshed by build_prompt's own TTL)."""
        try:
            return build_prompt(
                self.paths,
                self.config,
                workspace=self._workdir_for(topic_id),
                topic_id=topic_id,
            )
        except Exception:
            logger.exception("system prompt render failed; sending empty preamble")
            return ""

    def _extra_prompts(self) -> str:
        try:
            return load_extra_prompts(self.paths)
        except Exception:
            logger.exception("extra prompts load failed; skipping")
            return ""

    # ---- per-topic helpers -----------------------------------------------
    def _workdir_for(self, topic_id: int) -> Path:
        """Resolve the cursor-agent ``--workspace`` for ``topic_id``.

        Falls back to ``Path.home()`` when no per-topic state exists yet
        (newly-adopted topic, or a race where state was wiped).
        """
        return topic_state.workspace_for(self.paths, int(topic_id))

    def _chat_lock_for(self, topic_id: int) -> asyncio.Lock:
        """Return (lazily creating) the per-topic chat-session lock."""
        lock = self._chat_locks.get(topic_id)
        if lock is None:
            lock = asyncio.Lock()
            self._chat_locks[topic_id] = lock
        return lock

    def _router_lock_for(self, topic_id: int) -> asyncio.Lock:
        """Return (lazily creating) the per-topic router-session lock.

        Same rationale as :meth:`_chat_lock_for`: ``--resume <id>`` on
        the same session id from two concurrent invocations would
        interleave appends; the lock serialises them per-topic. Routing
        for different topics still runs in parallel.
        """
        lock = self._router_locks.get(topic_id)
        if lock is None:
            lock = asyncio.Lock()
            self._router_locks[topic_id] = lock
        return lock

    def _topic_thread_id(self, message: dict[str, Any]) -> int:
        """Extract the message's forum thread id (== adopted topic id)."""
        thread_id = message.get("message_thread_id")
        if isinstance(thread_id, int):
            return thread_id
        # Defensive: the home topic is the only place a non-thread message
        # could plausibly land in our supergroup, so route it there.
        return self.home_topic_id

    def _start_outbox_watcher(self, topic_id: int) -> None:
        """Spin up (idempotently) the per-topic outbox watcher.

        No-op when the agent's http client is not yet up (e.g. called from
        ``__init__``); ``run()`` boots watchers for every adopted topic
        right after the client opens, and ``_is_for_us`` calls this again
        on first adoption of a new topic at runtime.
        """
        if topic_id in self._outbox_watchers:
            return
        http = self._outbox_http
        if http is None:
            return
        watcher = OutboxWatcher(
            http=http,
            bot_token=self.bot_token,
            chat_id=self.chat_id,
            message_thread_id=int(topic_id),
            paths=self.paths,
            topic_id=int(topic_id),
        )
        watcher.start()
        self._outbox_watchers[int(topic_id)] = watcher

    # ---- topic header rendering -----------------------------------------
    @staticmethod
    def _short_workspace(path: str | Path) -> str:
        """Render an absolute path as ``~/foo`` when it lives under ``$HOME``.

        Plain absolute paths (outside home) are returned unchanged. Used
        as the workspace suffix in the Telegram topic title so the user
        can see at a glance what cwd this thread is bound to.
        """
        p = Path(path).expanduser()
        try:
            home = Path.home()
        except (OSError, RuntimeError):
            return str(p)
        try:
            rel = p.relative_to(home)
        except ValueError:
            return str(p)
        rel_str = str(rel)
        if rel_str in ("", "."):
            return "~"
        return f"~/{rel_str}"

    def _render_topic_name(
        self,
        *,
        name: Optional[str],
        base_title: Optional[str],
        workspace: str,
    ) -> str:
        """Compose the Telegram topic name as ``<name> · <base>@<workspace>``.

        ``<base>`` and ``<workspace>`` are joined with ``@`` (think
        ``user@host`` -- the agent is "running as <base> in <cwd>");
        the user-given ``<name>`` is the first ``·``-separated
        component. Missing components degrade gracefully:

        * ``name=None``  -> ``<base>@<workspace>``
        * ``base=None``  -> ``<name> · <workspace>``
        * both None      -> just ``<workspace>``

        Telegram caps forum topic names at 128 chars; the rendered name
        is ellipsis-truncated if needed.
        """
        short = self._short_workspace(workspace)
        if base_title:
            tail = f"{base_title}@{short}"
        else:
            tail = short
        out = f"{name} · {tail}" if name else tail
        if len(out) > 128:
            out = out[:127] + "…"
        return out

    async def _rename_topic_for_workspace(
        self,
        http: httpx.AsyncClient,
        topic_id: int,
        workspace: str,
    ) -> bool:
        """Push ``<name> · <base> · <workspace>`` into the topic title.

        Idempotent: skips the API call when the topic's cached ``title``
        already matches the desired name. Returns True iff Telegram
        confirmed the rename.
        """
        state = topic_state.load(self.paths, topic_id)
        new_name = self._render_topic_name(
            name=state.name if state else None,
            base_title=state.base_title if state else None,
            workspace=workspace,
        )
        if state is not None and state.title == new_name:
            return False
        try:
            resp = await self._api(
                http,
                "editForumTopic",
                chat_id=self.chat_id,
                message_thread_id=int(topic_id),
                name=new_name,
            )
        except httpx.HTTPError as exc:
            logger.warning(
                "editForumTopic network error (topic=%s): %s", topic_id, exc
            )
            return False
        if resp.status_code != 200:
            logger.warning(
                "editForumTopic HTTP %d (topic=%s): %s",
                resp.status_code, topic_id, resp.text[:200],
            )
            return False
        body = resp.json()
        if not body.get("ok"):
            logger.warning(
                "editForumTopic not ok (topic=%s): %r", topic_id, body
            )
            return False
        if state is not None:
            state.title = new_name
            try:
                topic_state.save(self.paths, state)
            except Exception:
                logger.exception(
                    "could not persist topic title cache (topic=%s)", topic_id
                )
        logger.info("renamed topic %s -> %r", topic_id, new_name)
        return True

    def _maybe_capture_topic_name(self, message: dict[str, Any]) -> None:
        """Pull the user-given name out of a ``forum_topic_created`` /
        ``forum_topic_edited`` service message and remember it.

        Always updates ``self._topic_name_cache`` (so the name survives
        the gap between create-event and the first @-mention that
        actually adopts the topic). Additionally persists immediately
        if the topic has already been adopted.

        Skips inputs that look like the agent's own rendered title
        (anything containing the ``" · "`` separator OR an embedded
        ``"@"`` between base and workspace) to avoid an echo loop with
        our own ``editForumTopic`` calls.
        """
        thread_id = message.get("message_thread_id")
        if not isinstance(thread_id, int):
            return
        raw_name: Optional[str] = None
        created = message.get("forum_topic_created")
        if isinstance(created, dict):
            raw = created.get("name")
            if isinstance(raw, str):
                raw_name = raw
        if raw_name is None:
            edited = message.get("forum_topic_edited")
            if isinstance(edited, dict):
                raw = edited.get("name")
                if isinstance(raw, str):
                    raw_name = raw
        if not raw_name:
            return
        cleaned = raw_name.strip()
        if not cleaned:
            return
        if " · " in cleaned or self._looks_like_our_title(cleaned):
            return
        # Cache unconditionally so a not-yet-adopted topic still gets
        # its name applied when ``_is_for_us`` later adopts it.
        self._topic_name_cache[thread_id] = cleaned
        if not topic_state.is_adopted(self.paths, thread_id):
            return
        try:
            updated = topic_state.set_name(self.paths, thread_id, cleaned)
        except Exception:
            logger.exception(
                "topic_state.set_name failed (topic=%s)", thread_id
            )
            return
        if updated is None:
            return
        logger.info(
            "captured topic name (topic=%s) -> %r", thread_id, cleaned
        )
        # Re-render so the new name component lands in the title.
        self._schedule_topic_rename(thread_id)

    def _looks_like_our_title(self, candidate: str) -> bool:
        """True if ``candidate`` is plausibly something we wrote ourselves.

        Defends against an echo loop: when we ``editForumTopic`` to
        ``"<name> · <base>@<ws>"``, Telegram emits a
        ``forum_topic_edited`` event back at us. The ``"·"`` check
        catches the three-component form; ``"<base>@~"`` form is also
        ours (no ``name`` component yet).
        """
        if " · " in candidate:
            return True
        # Pattern "<base>@<workspace_path>" — we only generate this when
        # base_title is set, and the suffix is an absolute or ~/ path.
        at = candidate.rfind("@")
        if at <= 0:
            return False
        suffix = candidate[at + 1 :]
        return suffix.startswith("~") or suffix.startswith("/")

    def _schedule_listening_notification(
        self,
        message: dict[str, Any],
        topic_id: int,
        *,
        first_adoption: bool,
    ) -> None:
        """Send a short ack bubble in response to an @-mention.

        ``first_adoption=True`` means the mention just adopted the topic;
        the ack tells the user where the workspace is pointed and how
        to change it. ``first_adoption=False`` is a re-ping in an
        already-adopted topic; the ack just confirms we're still
        listening so the user knows the @ wasn't dispatched as a prompt.
        """
        http = self._outbox_http
        if http is None:
            return
        workspace_short = self._short_workspace(self._workdir_for(topic_id))
        if first_adoption:
            text = (
                f"listening on this topic\n"
                f"workspace: {workspace_short}\n"
                f"send a message (no @ needed) or "
                f"`/workspace <path>` to change the cwd"
            )
        else:
            text = f"listening (workspace: {workspace_short})"

        async def _do_notify() -> None:
            try:
                await self._send_initial_bubble(http, message, text=text)
            except Exception:
                logger.exception(
                    "listening notification failed (topic=%s)", topic_id
                )

        task = asyncio.create_task(
            _do_notify(), name=f"listening-notify-{topic_id}"
        )
        self._tasks.add(task)
        task.add_done_callback(lambda t: self._tasks.discard(t))

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
        """True iff we should dispatch ``message`` to cursor-agent.

        Filters in priority order:

        1. Must land in our supergroup chat.
        2. Must come from a real user (not a bot, not us).
        3. Service messages (``forum_topic_created`` / ``forum_topic_edited``)
           are intercepted to capture the user-given topic name; they are
           never dispatched as prompts.
        4. Must have something we can act on (text or supported audio).
        5. @-mentions of our bot are treated as "ping me, I'm listening"
           rather than as prompts: we adopt the topic if needed, schedule
           an ack bubble, and return False so the message is NOT sent to
           cursor-agent. The user then sends their actual prompt as a
           plain (un-@'d) message.
        6. Otherwise: only dispatch when the topic is already adopted.
        """
        chat = message.get("chat") or {}
        if chat.get("id") != self.chat_id:
            return False

        # Service-message side channel: capture topic names regardless of
        # whether the message would otherwise be acted on.
        self._maybe_capture_topic_name(message)

        sender = message.get("from") or {}
        if sender.get("is_bot"):
            return False
        if self.bot_user_id is not None and sender.get("id") == self.bot_user_id:
            return False
        if not (
            (message.get("text") or "").strip()
            or self._extract_audio(message) is not None
        ):
            return False

        thread_id = message.get("message_thread_id")
        if not isinstance(thread_id, int):
            # Non-topic messages in the supergroup are out of scope.
            return False

        is_mention = self._message_mentions_us(message)

        if thread_id in self._adopted:
            if is_mention:
                # Already adopted; user @-mentioned us. Ack and skip
                # dispatching so the @ doesn't become a prompt.
                self._schedule_listening_notification(
                    message, thread_id, first_adoption=False
                )
                logger.info(
                    "ignoring @%s ping in already-adopted topic %s "
                    "(sent listening ack)",
                    self.bot_username, thread_id,
                )
                return False
            return True

        if not is_mention:
            return False

        # First contact: mention adopts the topic but does NOT dispatch
        # the triggering message; we ack instead so the user knows we
        # heard them and can send their actual prompt next. Drain the
        # in-memory name cache (populated by an earlier
        # ``forum_topic_created`` update) so the user-given topic name
        # lands on disk as part of the same write.
        cached_name = self._topic_name_cache.pop(thread_id, None)
        try:
            topic_state.adopt(
                self.paths,
                thread_id,
                base_title=self.machine,
                name=cached_name,
            )
        except Exception:
            logger.exception("topic_state.adopt failed for %s", thread_id)
            return False
        self._adopted.add(thread_id)
        self._start_outbox_watcher(thread_id)
        logger.info(
            "adopted new topic %s via @%s mention from %s (name=%r)",
            thread_id, self.bot_username,
            sender.get("username") or sender.get("id"),
            cached_name,
        )
        # Rename the topic header to show the (default) workspace as a
        # side-channel hint that arbos is now driving this thread, then
        # ack so the user sees we're listening.
        self._schedule_topic_rename(thread_id)
        self._schedule_listening_notification(
            message, thread_id, first_adoption=True
        )
        return False

    def _schedule_topic_rename(self, topic_id: int) -> None:
        """Fire-and-forget ``editForumTopic`` for ``topic_id`` from sync code.

        Used from ``_is_for_us`` (which can't ``await``). The task is
        added to ``self._tasks`` so the run-loop's shutdown drain
        cancels it cleanly if we're stopping.
        """
        http = self._outbox_http  # captured in run() before polling starts
        if http is None:
            return
        workspace = str(self._workdir_for(topic_id))

        async def _do_rename() -> None:
            try:
                await self._rename_topic_for_workspace(http, topic_id, workspace)
            except Exception:
                logger.exception(
                    "scheduled topic rename failed (topic=%s)", topic_id
                )

        task = asyncio.create_task(
            _do_rename(), name=f"rename-topic-{topic_id}"
        )
        self._tasks.add(task)
        task.add_done_callback(lambda t: self._tasks.discard(t))

    def _message_mentions_us(self, message: dict[str, Any]) -> bool:
        """True iff ``message`` contains a Telegram mention of our bot.

        Honours both ``mention`` entities (``@bot_username`` literal in the
        text/caption) and ``text_mention`` entities (a tagged user id, used
        when the username collides with someone else or is hidden).
        """
        target = f"@{self.bot_username}".lower() if self.bot_username else ""
        for field in ("text", "caption"):
            text = message.get(field) or ""
            if not text:
                continue
            entities_key = "entities" if field == "text" else "caption_entities"
            for entity in message.get(entities_key) or []:
                if not isinstance(entity, dict):
                    continue
                etype = entity.get("type")
                if etype == "mention" and target:
                    offset = entity.get("offset")
                    length = entity.get("length")
                    if isinstance(offset, int) and isinstance(length, int):
                        chunk = text[offset : offset + length]
                        if chunk.lower() == target:
                            return True
                elif etype == "text_mention":
                    user = entity.get("user") or {}
                    if (
                        self.bot_user_id is not None
                        and user.get("id") == self.bot_user_id
                    ):
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
            "message_thread_id": self._topic_thread_id(message),
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
        edit_interval: Optional[float] = None,
    ) -> Bubble:
        """Wrap a Telegram message id in a ``Bubble`` with our journal hook.

        Centralised so every Bubble in the agent automatically rolls its
        run-journal entry onto each continuation message_id when a long
        answer overflows Telegram's 4096-char per-message cap. Callers
        that override ``chat_id`` / ``message_thread_id`` are the rare
        cross-thread bubbles (e.g. confirmations posted into a different
        topic); everyone else uses this install's primary topic.

        ``edit_interval`` overrides the per-bubble editMessageText floor.
        Rollout bubbles pass a tighter value (see ROLLOUT_EDIT_INTERVAL)
        so the supervisor's live timer prefix can advance every two
        seconds; static bubbles (confirmations, restart, voice) inherit
        the safe default.
        """

        async def _on_continuation(prev_id: int, new_id: int) -> None:
            # Mirror the journal entry onto the new message id so a
            # reply that lands on either part resolves to the same run.
            try:
                self._runs.link_alias(prev_id, new_id)
            except Exception:
                logger.exception("runjournal link_alias failed (swallowed)")

        kwargs: dict[str, Any] = {
            "http": http,
            "bot_token": self.bot_token,
            "chat_id": chat_id if chat_id is not None else self.chat_id,
            "message_thread_id": (
                message_thread_id
                if message_thread_id is not None
                else self.home_topic_id
            ),
            "message_id": message_id,
            "on_continuation": _on_continuation,
        }
        if edit_interval is not None:
            kwargs["edit_interval"] = edit_interval
        return Bubble(**kwargs)

    async def _post_summary_bubble(
        self,
        http: httpx.AsyncClient,
        *,
        chat_id: int,
        message_thread_id: int,
        rollout_message_id: int,
        text: str,
    ) -> Optional[int]:
        """Send a fresh Telegram message in ``message_thread_id`` containing
        the supervisor's past-tense recap. Posted as a reply to the rollout
        bubble so the two are visually linked, then aliased into the
        rollout's run-journal entry so a reply to either resolves to the
        same run.

        Returns the new message id, or ``None`` if the send failed.
        """
        body = (text or "").strip()
        if not body:
            return None
        params: dict[str, Any] = {
            "chat_id": chat_id,
            "message_thread_id": message_thread_id,
            "text": body,
            "disable_web_page_preview": True,
            "reply_parameters": {
                "message_id": rollout_message_id,
                "allow_sending_without_reply": True,
            },
        }
        try:
            resp = await self._api(http, "sendMessage", **params)
        except httpx.HTTPError as exc:
            logger.warning("summary sendMessage network error: %s", exc)
            return None
        if resp.status_code != 200:
            logger.warning(
                "summary sendMessage HTTP %d: %s",
                resp.status_code,
                resp.text[:200],
            )
            return None
        result = (resp.json() or {}).get("result") or {}
        new_id = result.get("message_id")
        if not new_id:
            return None
        try:
            self._runs.link_alias(rollout_message_id, int(new_id))
        except Exception:
            logger.exception("runjournal link_alias for summary bubble failed")
        return int(new_id)

    async def _run_supervised(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        bubble: Bubble,
        runner: CursorRunner,
        *,
        initial_text: str = INITIAL_BUBBLE_TEXT,
        on_final_capture: Optional[Callable[[str], Awaitable[None]]] = None,
    ) -> None:
        """Run ``runner`` under a Supervisor that owns the rollout bubble's
        terminal line, then post a separate summary bubble with the LLM
        recap.

        ``on_final_capture`` is an optional async hook called with the
        worker's raw final text *before* the supervisor finalises the
        rollout. ``/plan`` uses this to capture the plan text for phase 2.

        On cancel / crash the rollout's terminal line is appended via
        ``mark_interrupted`` / ``mark_crashed`` and no summary bubble is
        sent. On success we post a fresh Telegram message in the same
        thread (as a reply to the rollout) containing the supervisor's
        past-tense recap.
        """
        recap: Optional[str] = None
        # The runner's prompt drives the supervisor's "Starting <task>"
        # hint; trim to keep the LLM call cheap.
        task_prompt = (runner.prompt or "").strip()[:1500] or None
        async with self._make_supervisor(
            bubble, initial_text, task_prompt=task_prompt
        ) as sup:
            async def _on_final(text: str) -> None:
                if on_final_capture is not None:
                    try:
                        await on_final_capture(text)
                    except Exception:
                        logger.exception("on_final_capture raised")
                await sup.on_final(text)
            try:
                await runner.run(on_update=sup.on_update, on_final=_on_final)
            except asyncio.CancelledError:
                await sup.mark_interrupted()
                raise
            except Exception as exc:
                await sup.mark_crashed(str(exc))
                raise
            try:
                recap = await sup.summarise_final()
            except Exception:
                logger.exception("supervisor summarise_final crashed")
                recap = None

        if recap:
            try:
                await self._post_summary_bubble(
                    http,
                    chat_id=self.chat_id,
                    message_thread_id=self._topic_thread_id(message),
                    rollout_message_id=bubble.message_id,
                    text=recap,
                )
            except Exception:
                logger.exception("posting summary bubble failed (swallowed)")

    async def _run_smart_router(
        self,
        *,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        prompt: str,
        bubble: Bubble,
        ctx: RouterContext,
        topic_id: int,
    ) -> tuple[Optional[RouterDecision], Optional[CursorRunner]]:
        """Spawn cursor-agent in agent mode for a routing decision and stream
        live status into ``bubble`` via the supervisor.

        Returns ``(decision, runner)``. ``decision`` is the parsed
        :class:`RouterDecision` (slash + recap text) on success, or
        ``None`` on any failure (spawn error, resume-session miss,
        cursor-agent crash) -- the caller treats ``None`` as "delegate
        to the worker" so the user always still gets a response.
        ``runner`` is returned even on failure so the caller can read
        ``runner.final_text`` for the run-journal record_finish entry.

        Per-topic serialised on the SmartRouter lock because
        ``--resume <id>`` from two parallel calls would interleave the
        single chat thread.
        """
        async with self._router_lock_for(topic_id):
            resume_id = self._router_sessions.get(topic_id)
            workdir = self._workdir_for(topic_id)
            system_prompt = self._router.build_system_prompt(ctx)

            runner = CursorRunner(
                prompt=prompt,
                workdir=workdir,
                model=ROUTER_MODEL,
                system_prompt=system_prompt,
                resume_session=resume_id,
            )

            task_prompt = (prompt or "").strip()[:1500] or None
            try:
                async with self._make_supervisor(
                    bubble, ROUTING_BUBBLE_TEXT, task_prompt=task_prompt
                ) as sup:
                    try:
                        await runner.run(
                            on_update=sup.on_update, on_final=sup.on_final
                        )
                    except asyncio.CancelledError:
                        await sup.mark_interrupted()
                        raise
                    except Exception as exc:
                        await sup.mark_crashed(str(exc))
                        logger.exception("smart router cursor-agent crashed")
                        return (None, runner)
            except asyncio.CancelledError:
                raise

            # Stale --resume recovery: if the stored router session id is
            # gone upstream, drop it so the next call starts fresh. We
            # do NOT retry inside this hop -- the user's message is the
            # routing trigger, and on a missed resume we conservatively
            # delegate so the worker still gets the message verbatim.
            if runner.resume_failed and resume_id:
                logger.warning(
                    "smart router resume of %s (topic=%s) failed; "
                    "clearing and falling through to worker",
                    resume_id, topic_id,
                )
                self._router_sessions[topic_id] = None
                try:
                    session_store.clear_router(self.paths, topic_id)
                except Exception:
                    logger.exception(
                        "session_store.clear_router crashed for topic %s",
                        topic_id,
                    )
                return (None, runner)

            new_id = runner.session_id
            if new_id and new_id != self._router_sessions.get(topic_id):
                self._router_sessions[topic_id] = new_id
                try:
                    session_store.save_router(
                        self.paths,
                        topic_id,
                        ChatSession.now(session_id=new_id, model=ROUTER_MODEL),
                    )
                    logger.info(
                        "persisted new router session id: %s (topic=%s)",
                        new_id, topic_id,
                    )
                except OSError as exc:
                    logger.warning(
                        "could not persist router session %s (topic=%s): %s",
                        new_id, topic_id, exc,
                    )

            return (self._router.parse_decision(runner.final_text), runner)

    def _make_supervisor(
        self,
        bubble: Bubble,
        initial_text: str,
        *,
        task_prompt: Optional[str] = None,
    ) -> Supervisor:
        """Wrap ``bubble`` in a Supervisor that summarises the worker's noisy
        snapshot stream into a single short status line every couple of
        seconds, with an elapsed-time heartbeat and stall detection so the
        bubble always feels alive. When the supervisor is disabled (no
        key, or ``ARBOS_SUPERVISOR_DISABLE=1``), the wrapper still drives
        the heartbeat but uses a deterministic Python summarizer over the
        worker's rendered snapshot instead of an LLM call.

        ``initial_text`` is normalized into a short rollout-friendly word
        (``starting`` / ``planning`` / ``implementing``); the legacy
        ``thinking...`` style placeholders are only used as the
        pre-supervisor ``sendMessage`` text the bubble briefly shows
        before ``__aenter__`` runs.
        """
        # Map the bubble's pre-supervisor placeholders into rollout-style
        # capitalized words so the first rollout line reads as
        # "0:00  Starting <task>" (with the task hint filled in
        # asynchronously by the supervisor's task-hint LLM call).
        initial = {
            INITIAL_BUBBLE_TEXT: "Starting",
            PLAN_INITIAL_BUBBLE_TEXT: "Planning",
            IMPL_INITIAL_BUBBLE_TEXT: "Implementing",
            ROUTING_BUBBLE_TEXT: "Routing",
        }.get(initial_text, "Starting")
        key = "" if SUPERVISOR_DISABLE else self.openai_api_key
        return Supervisor(
            bubble=bubble,
            openai_api_key=key,
            model=SUPERVISOR_MODEL,
            interval=SUPERVISOR_INTERVAL,
            max_chars=SUPERVISOR_MAX_CHARS,
            final_max_chars=SUPERVISOR_FINAL_MAX_CHARS,
            stall_hint_after=SUPERVISOR_STALL_HINT_AFTER,
            stall_warn_after=SUPERVISOR_STALL_WARN_AFTER,
            initial_text=initial,
            task_prompt=task_prompt,
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

    def _match_abort_command(self, prompt: str) -> bool:
        """True if ``prompt`` is ``/abort`` (optionally ``@<bot>``).

        Trailing arguments are ignored -- /abort is parameterless.
        """
        head = prompt.partition(" ")[0].lower()
        candidates = {"/abort"}
        if self.bot_username:
            candidates.add(f"/abort@{self.bot_username.lower()}")
        return head in candidates

    def _match_cron_command(self, prompt: str) -> Optional[str]:
        """Return the trailing argument if ``prompt`` is a ``/cron`` command,
        else None. The argument may be empty (bare ``/cron`` lists)."""
        head, _, rest = prompt.partition(" ")
        head_lower = head.lower()
        candidates = {"/cron"}
        if self.bot_username:
            candidates.add(f"/cron@{self.bot_username.lower()}")
        if head_lower not in candidates:
            return None
        return rest.strip()

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

    def _match_workspace_command(self, prompt: str) -> Optional[str]:
        """Return the trailing argument if ``prompt`` is ``/workspace`` (or
        ``/cwd`` alias), else None.

        Empty argument means "show the current workspace"; anything else is
        a path to set as the topic's cursor-agent ``--workspace``.
        """
        head, _, rest = prompt.partition(" ")
        head_lower = head.lower()
        candidates = {"/workspace", "/cwd"}
        if self.bot_username:
            uname = self.bot_username.lower()
            candidates.add(f"/workspace@{uname}")
            candidates.add(f"/cwd@{uname}")
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
            voice_bubble = self._make_bubble(
                http,
                sent["message_id"],
                message_thread_id=self._topic_thread_id(message),
            )

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

        cron_arg = self._match_cron_command(prompt)
        if cron_arg is not None:
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            logger.info("dispatching /cron from %s (args=%r)", sender, cron_arg[:80])
            await self._handle_cron_command(http, message, cron_arg)
            return

        workspace_arg = self._match_workspace_command(prompt)
        if workspace_arg is not None:
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            logger.info(
                "dispatching /workspace from %s (arg=%r)",
                sender, workspace_arg[:120],
            )
            await self._handle_workspace_command(http, message, workspace_arg)
            return

        if self._match_reset_command(prompt):
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            topic_id = self._topic_thread_id(message)
            if self._skip_confirm:
                logger.info("dispatching /reset from %s (confirm skipped)", sender)
                await self._handle_reset_command(http, message)
            else:
                logger.info("gating /reset from %s behind Confirm/Cancel", sender)
                await self._prompt_confirmation(
                    http,
                    message,
                    label="/reset",
                    description="wipe the persistent chat session for this topic",
                    action=lambda b, _tid=topic_id: self._handle_reset_action(b, topic_id=_tid),
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
            topic_id = self._topic_thread_id(message)
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
                    action=lambda b, _tid=topic_id: self._handle_restart_action(b, thread_id=_tid),
                )
            return

        if self._match_abort_command(prompt):
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            if self._skip_confirm:
                logger.info("dispatching /abort from %s (confirm skipped)", sender)
                await self._handle_abort_command(http, message)
            else:
                logger.info("gating /abort from %s behind Confirm/Cancel", sender)
                await self._prompt_confirmation(
                    http,
                    message,
                    label="/abort",
                    description=(
                        "cancel all in-flight handlers (cursor-agent runs, "
                        "/plan, /update, …); crons keep running"
                    ),
                    action=lambda b: self._handle_abort_action(b),
                )
            return

        # Front-line SmartRouter: cursor-agent in agent mode against
        # this topic's --resume router session, so the routing decision
        # has full chat-history context. The agent ends its reply with
        # one of a small slash-command set (/cron, /restart, /workspace,
        # /update, /plan, /delegate) which we parse + dispatch via the
        # existing handlers (_router_do_* below).
        #
        # ONE bubble per user message wherever possible:
        #   - text answer:    bubble's final text IS the answer.
        #   - action slash:   bubble's final text IS the recap + decision
        #                     ("Scheduling 5-min disk check.\n→ /cron 5 ...");
        #                     the dispatched handler creates its own ack bubble.
        #   - /delegate:      bubble's final text is the recap; the worker
        #                     spawns a separate rollout + summary pair so
        #                     its progress is visible distinctly.
        if not self._router.enabled:
            logger.info(
                "smart router disabled; dispatching message from %s straight "
                "to worker (len=%d)", sender, len(prompt),
            )
            await self._dispatch_chat_worker(
                http=http,
                message=message,
                prompt=prompt,
                voice_bubble=voice_bubble,
                reply_block=reply_block,
            )
            return

        topic_id = self._topic_thread_id(message)
        ctx = self._build_router_ctx(http, message)

        if voice_bubble is not None:
            bubble = voice_bubble
            await bubble.update(ROUTING_BUBBLE_TEXT)
        else:
            sent = await self._send_initial_bubble(
                http, message, text=ROUTING_BUBBLE_TEXT,
            )
            if sent is None:
                return
            bubble = self._make_bubble(
                http,
                sent["message_id"],
                message_thread_id=topic_id,
                edit_interval=ROLLOUT_EDIT_INTERVAL,
            )

        # Journal the routing bubble so a reply that lands while it's
        # in-flight resolves to this run. Records ROUTER_MODEL so the
        # journal reflects which model actually produced the recap; if
        # the router delegates, the worker's own bubble (with model=
        # CHAT_MODEL) is journaled separately.
        self._runs.record_start(
            bubble.message_id,
            kind="chat",
            trigger_message_id=message.get("message_id"),
            trigger_text=prompt,
            trigger_sender=self._trigger_sender_label(message),
            model=ROUTER_MODEL,
        )

        # Reply-context is grounding for resolving "no, the second one"
        # / "did that finish?" -- equally useful to the router and the
        # worker, so we feed it to both via the same fenced block.
        router_prompt = (
            (reply_block + "\n\n" + prompt) if reply_block else prompt
        )

        decision: Optional[RouterDecision] = None
        router_runner: Optional[CursorRunner] = None
        try:
            decision, router_runner = await self._run_smart_router(
                http=http,
                message=message,
                prompt=router_prompt,
                bubble=bubble,
                ctx=ctx,
                topic_id=topic_id,
            )
        except asyncio.CancelledError:
            await bubble.aclose()
            self._record_chat_finish(bubble, "interrupted", router_runner)
            raise
        except Exception:
            logger.exception("smart router crashed; falling through to worker")

        # Recover-by-delegate: if the router never produced a decision
        # we want the worker to actually answer the user. The routing
        # bubble has been finalised by the supervisor with its terminal
        # "Done" line; surface a short note and spawn a fresh worker
        # bubble so the user has clear visual progress.
        if decision is None:
            try:
                await bubble.finalize(
                    "router unavailable; handing off to the worker…"
                )
            except Exception:
                logger.exception("finalising router-failure bubble (swallowed)")
            finally:
                await bubble.aclose()
            self._record_chat_finish(bubble, "error", router_runner)
            await self._dispatch_chat_worker(
                http=http,
                message=message,
                prompt=prompt,
                voice_bubble=None,
                reply_block=reply_block,
            )
            return

        # No-slash branch: the router answered the user directly; the
        # answer text becomes the bubble's final state, no extra bubble.
        if decision.slash is None:
            answer = (
                decision.recap.strip()
                or decision.raw_final.strip()
                or "(no response)"
            )
            try:
                await bubble.finalize(answer)
            except Exception:
                logger.exception("finalising router answer bubble (swallowed)")
            finally:
                await bubble.aclose()
            self._record_chat_finish(bubble, "ok", router_runner)
            logger.info(
                "smart router answered %s directly (len=%d)", sender, len(answer),
            )
            return

        cmd, args = decision.slash
        logger.info(
            "smart router → %s (sender=%s, args=%r)", cmd, sender, args[:80],
        )

        # Render the routing bubble's *final* state as recap + decision
        # marker. For /delegate the recap is usually a one-liner ("I'll
        # forward this to the worker"); we surface it so the user can see
        # WHY the router delegated.
        recap = decision.recap.strip()
        ack_line = f"→ {cmd}{(' ' + args) if args else ''}"
        display = (recap + "\n\n" + ack_line) if recap else ack_line
        try:
            await bubble.finalize(display)
        except Exception:
            logger.exception("finalising router decision bubble (swallowed)")
        finally:
            await bubble.aclose()
        self._record_chat_finish(bubble, "ok", router_runner)

        if cmd == DELEGATE_SLASH:
            # /delegate: hand off to the heavy worker on a fresh bubble.
            # The routing bubble stands as the audit trail of the routing
            # decision; the worker bubble carries the actual progress +
            # answer so the two are visually distinct.
            await self._dispatch_chat_worker(
                http=http,
                message=message,
                prompt=prompt,
                voice_bubble=None,
                reply_block=reply_block,
            )
            return

        # Action slash: dispatch via existing handlers (which own their
        # ack bubble lifecycle).
        try:
            outcome = await self._router.dispatch(cmd, args, ctx)
        except Exception:
            logger.exception(
                "router dispatch of %s crashed; falling through to worker", cmd,
            )
            outcome = RouterOutcome.DELEGATE

        if outcome is RouterOutcome.HANDLED:
            return

        # Unknown slash (defensive): treat as delegate.
        await self._dispatch_chat_worker(
            http=http,
            message=message,
            prompt=prompt,
            voice_bubble=None,
            reply_block=reply_block,
        )

    async def _dispatch_chat_worker(
        self,
        *,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        prompt: str,
        voice_bubble: Optional[Bubble],
        reply_block: str,
    ) -> None:
        """Run the heavy chat worker for ``prompt`` in a fresh rollout bubble.

        Factored out of ``_handle_message`` so both the SmartRouter
        delegation path and the router-disabled bypass path share one
        implementation. Reuses ``voice_bubble`` if supplied (so the
        already-visible voice transcription bubble continues into the
        worker), otherwise opens a new ``thinking...`` bubble.
        """
        sender = self._trigger_sender_label(message)
        if voice_bubble is not None:
            bubble = voice_bubble
            await bubble.update(INITIAL_BUBBLE_TEXT)
        else:
            sent = await self._send_initial_bubble(http, message)
            if sent is None:
                return
            bubble = self._make_bubble(
                http,
                sent["message_id"],
                message_thread_id=self._topic_thread_id(message),
                edit_interval=ROLLOUT_EDIT_INTERVAL,
            )

        self._runs.record_start(
            bubble.message_id,
            kind="chat",
            trigger_message_id=message.get("message_id"),
            trigger_text=prompt,
            trigger_sender=sender,
            model=CHAT_MODEL,
        )

        await self._run_persistent_chat(
            http=http,
            message=message,
            prompt=prompt,
            bubble=bubble,
            reply_context=reply_block,
        )

    # ---- router glue ----------------------------------------------------
    # The five ``_router_do_*`` methods below are thin shims the front-line
    # router calls into; they reuse existing slash-command handlers so the
    # router never reimplements behaviour, only routes to it. ``do_say``
    # is the one-shot bubble used by the router for clarifications and
    # arg-validation errors.

    def _build_router_ctx(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> RouterContext:
        """Construct the per-message :class:`RouterContext`.

        Each bound ``do_*`` closes over ``http`` + ``message`` so the
        :mod:`router` module never sees Telegram primitives directly. The
        topic id and workspace are also captured here so the router's
        system prompt can mention them (it does NOT need the full Arbos
        system context).
        """
        topic_id = self._topic_thread_id(message)
        workspace = self._workdir_for(topic_id)
        return RouterContext(
            machine_name=self.machine,
            topic_id=topic_id,
            workspace=workspace,
            do_workspace=lambda p: self._router_do_workspace(http, message, p),
            do_restart_with_confirm=lambda: self._router_do_restart(http, message),
            do_update_with_confirm=lambda: self._router_do_update(http, message),
            do_add_cron=lambda mins, prompt: self._router_do_add_cron(
                http, message, mins, prompt
            ),
            do_plan=lambda req: self._router_do_plan(http, message, req),
            do_say=lambda text: self._send_short_bubble(http, message, text),
        )

    async def _router_do_workspace(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        path: Path,
    ) -> None:
        """Forward to ``_handle_workspace_command`` with a resolved path.

        The handler already validates existence + dir-ness, persists the
        new workspace, clears the topic's chat session, re-renders
        PROMPT.md, renames the topic, and finalises an ack bubble. We
        pass ``str(path)``; the handler re-resolves (idempotent on an
        already-resolved path).
        """
        await self._handle_workspace_command(http, message, str(path))

    async def _router_do_restart(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        """Restart, gated through the existing Confirm/Cancel keyboard.

        Honours ``ARBOS_SKIP_CONFIRM`` exactly like the literal
        ``/restart`` slash command does -- the router never bypasses
        the confirmation gate that the user explicitly opted into.
        """
        topic_id = self._topic_thread_id(message)
        if self._skip_confirm:
            await self._handle_restart_command(http, message)
            return
        await self._prompt_confirmation(
            http,
            message,
            label="/restart",
            description="pm2 reload this machine (no code changes)",
            action=lambda b, _tid=topic_id: self._handle_restart_action(b, thread_id=_tid),
        )

    async def _router_do_update(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        """Update from ``origin/main``, gated through Confirm/Cancel.

        Mirrors the literal ``/update`` slash-command path including the
        ``ARBOS_SKIP_CONFIRM`` bypass.
        """
        if self._skip_confirm:
            await self._handle_update_command(http, message)
            return
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

    async def _router_do_add_cron(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        mins: int,
        prompt: str,
    ) -> None:
        """Schedule a cron via the existing ``/cron <mins> <prompt>`` path.

        Format-and-forward keeps the cron-add behaviour (immediate first
        run, journaling, ack bubble shape) identical to the literal
        slash command -- the only difference is who invoked it.
        """
        composed = f"{int(mins)} {prompt}".strip()
        await self._handle_cron_command(http, message, composed)

    async def _router_do_plan(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        request: str,
    ) -> None:
        """Run the 2-phase ``/plan`` flow on a router-classified request.

        The reply-context block is the same one ``_handle_message``
        already computed above; we recompute here to avoid plumbing it
        through the router (cheap; the journal lookup is local).
        """
        reply_block = self._build_reply_context_block(message, request)
        await self._handle_plan_command(
            http, message, request, reply_context=reply_block
        )

    async def _run_persistent_chat(
        self,
        *,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        prompt: str,
        bubble: Bubble,
        reply_context: str = "",
    ) -> None:
        """Spawn cursor-agent against THIS topic's persistent chat.

        Serialised on a per-topic lock because cursor-agent's ``--resume``
        appends to a single chat thread; concurrent appends to the *same*
        chat would interleave. Different topics run in parallel.
        Auto-recovers from a stale/missing session by clearing the stored
        id and retrying once without ``--resume``.

        ``reply_context``, if non-empty, is prepended to ``prompt`` as a
        ``<<<REPLY_CONTEXT>>>``-fenced block so the model can ground "no,
        the second one" / "did that finish?" against a specific prior run.

        On success we post a separate "summary" bubble in the same thread
        (a reply to the rollout bubble) containing the supervisor's
        past-tense recap. On cancel/crash the rollout's terminal line
        (``interrupted at ...`` / ``crashed at ...``) is enough; no
        summary bubble is sent.
        """
        status = "ok"
        topic_id = self._topic_thread_id(message)
        async with self._chat_lock_for(topic_id):
            runner: Optional[CursorRunner] = None
            try:
                runner = await self._do_persistent_chat_run(
                    http=http,
                    message=message,
                    prompt=prompt,
                    bubble=bubble,
                    reply_context=reply_context,
                )
            except asyncio.CancelledError:
                status = "interrupted"
                # Rollout's terminal line was already appended by the
                # supervisor inside _do_persistent_chat_run; nothing else
                # to do at this layer.
                await bubble.aclose()
                self._record_chat_finish(bubble, status, runner)
                raise
            except Exception:
                status = "error"
                logger.exception("cursor-agent run crashed")
                # Rollout terminal line already appended by supervisor
                # (mark_crashed). Nothing else to surface.
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
        http: httpx.AsyncClient,
        message: dict[str, Any],
        prompt: str,
        bubble: Bubble,
        reply_context: str = "",
    ) -> CursorRunner:
        topic_id = self._topic_thread_id(message)
        resume_id = self._chat_sessions.get(topic_id)
        workdir = self._workdir_for(topic_id)
        # The REPLY_CONTEXT block is conceptually system-level
        # instructions for resolving a referent, but cursor-agent has no
        # dedicated channel for it; we tack it onto the user prompt with
        # explicit fences so the model treats it as authoritative.
        full_prompt = (
            (reply_context + "\n\n" + prompt) if reply_context else prompt
        )

        runner = CursorRunner(
            prompt=full_prompt,
            workdir=workdir,
            model=CHAT_MODEL,
            system_prompt=self._system_prompt(topic_id),
            extra_prompts=self._extra_prompts(),
            resume_session=resume_id,
        )
        await self._run_supervised(http, message, bubble, runner)

        if runner.resume_failed and resume_id:
            logger.warning(
                "resume of chat %s (topic=%s) failed; clearing and starting fresh",
                resume_id, topic_id,
            )
            self._chat_sessions[topic_id] = None
            session_store.clear(self.paths, topic_id)
            runner = CursorRunner(
                prompt=full_prompt,
                workdir=workdir,
                model=CHAT_MODEL,
                system_prompt=self._system_prompt(topic_id),
                extra_prompts=self._extra_prompts(),
            )
            await self._run_supervised(http, message, bubble, runner)

        new_id = runner.session_id
        if new_id and new_id != self._chat_sessions.get(topic_id):
            self._chat_sessions[topic_id] = new_id
            try:
                session_store.save(
                    self.paths,
                    topic_id,
                    ChatSession.now(session_id=new_id, model=CHAT_MODEL),
                )
                logger.info(
                    "persisted new chat session id: %s (topic=%s)", new_id, topic_id,
                )
            except OSError as exc:
                logger.warning(
                    "could not persist chat session %s (topic=%s): %s",
                    new_id, topic_id, exc,
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

    async def _handle_workspace_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        arg: str,
    ) -> None:
        """Set (or show) the cursor-agent ``--workspace`` for THIS topic.

        ``/workspace`` (no arg)        -> show current workspace
        ``/workspace ~/projA``        -> set workspace, clear chat session

        Tilde + relative paths are expanded against the agent's home; the
        target must already exist and be a directory. Setting the workspace
        clears the topic's persistent cursor-agent chat session because a
        cwd change typically means the user has moved to a different
        project, and a stale session would be confusing.
        """
        topic_id = self._topic_thread_id(message)
        arg = (arg or "").strip()

        if not arg:
            current = self._workdir_for(topic_id)
            await self._send_short_bubble(
                http,
                message,
                f"workspace: {current}\nusage: /workspace <path>",
            )
            return

        try:
            path = Path(arg).expanduser().resolve()
        except (OSError, RuntimeError) as exc:
            await self._send_short_bubble(
                http, message, f"could not resolve {arg!r}: {exc}",
            )
            return
        if not path.exists():
            await self._send_short_bubble(
                http, message, f"no such directory: {path}",
            )
            return
        if not path.is_dir():
            await self._send_short_bubble(
                http, message, f"not a directory: {path}",
            )
            return

        try:
            topic_state.set_workspace(self.paths, topic_id, str(path))
        except OSError as exc:
            await self._send_short_bubble(
                http, message, f"could not save workspace: {exc}",
            )
            return

        # cwd change == new project context. Wipe the persistent chat
        # session for this topic so the model isn't anchored on the
        # previous workspace's history.
        cleared_session: Optional[str] = None
        async with self._chat_lock_for(topic_id):
            cleared_session = self._chat_sessions.get(topic_id)
            self._chat_sessions[topic_id] = None
            try:
                session_store.clear(self.paths, topic_id)
            except Exception:
                logger.exception(
                    "session_store.clear crashed for topic %s", topic_id
                )

        # Re-render PROMPT.md with the new workspace so the next message's
        # system preamble reflects the change immediately (rather than
        # waiting for the per-topic cache to expire).
        try:
            build_prompt(
                self.paths,
                self.config,
                workspace=path,
                topic_id=topic_id,
                force=True,
            )
        except Exception:
            logger.exception(
                "post-/workspace PROMPT.md re-render failed (swallowed)"
            )

        # Push the new workspace into the Telegram topic title so the
        # header reflects what the agent is bound to. Best-effort: a
        # rename failure (e.g. revoked topic-management right) is just
        # logged; the workspace is still set in our own state.
        try:
            await self._rename_topic_for_workspace(http, topic_id, str(path))
        except Exception:
            logger.exception(
                "topic rename after /workspace failed (topic=%s)", topic_id
            )

        ack = f"workspace set: {path}"
        if cleared_session:
            ack += f"\nstarted a fresh chat for this topic (cleared {cleared_session})"
        else:
            ack += "\n(no prior chat session to clear)"
        await self._send_short_bubble(http, message, ack)
        logger.info(
            "topic %s workspace -> %s (session cleared: %s)",
            topic_id, path, bool(cleared_session),
        )

    async def _handle_reset_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        """Wipe THIS topic's persistent chat session and ack with a bubble."""
        sent = await self._send_initial_bubble(http, message, text="resetting…")
        if sent is None:
            return
        self._journal_command_start(
            sent["message_id"],
            kind="reset",
            message=message,
            trigger_text="/reset",
        )
        topic_id = self._topic_thread_id(message)
        bubble = self._make_bubble(
            http, sent["message_id"], message_thread_id=topic_id
        )
        try:
            await self._handle_reset_action(bubble, topic_id=topic_id)
        finally:
            await bubble.aclose()
            self._journal_command_finish(
                bubble.message_id,
                status="ok",
                final_text=bubble.last_sent or "",
            )

    async def _handle_reset_action(
        self, bubble: Bubble, *, topic_id: int
    ) -> None:
        """Reset behaviour for ``topic_id``, given an already-sent bubble.

        Wipes BOTH the worker chat session and the SmartRouter session so
        ``/reset`` returns the topic to a fully fresh state -- the router
        no longer remembers prior decisions and the worker no longer
        remembers prior coding context. Both locks are taken so we don't
        race an in-flight router or worker run.
        """
        async with self._chat_lock_for(topic_id):
            prev = self._chat_sessions.get(topic_id)
            removed = session_store.clear(self.paths, topic_id)
            self._chat_sessions[topic_id] = None
        async with self._router_lock_for(topic_id):
            prev_router = self._router_sessions.get(topic_id)
            removed_router = session_store.clear_router(self.paths, topic_id)
            self._router_sessions[topic_id] = None

        bits: list[str] = []
        if prev:
            bits.append(f"cleared chat session {prev}")
        elif removed:
            bits.append("cleared stale chat session file")
        if prev_router:
            bits.append(f"cleared router session {prev_router}")
        elif removed_router:
            bits.append("cleared stale router session file")
        if bits:
            text = "started a fresh chat for this topic (" + ", ".join(bits) + ")"
        else:
            text = "no chat or router session to reset; next message will start one"
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
        bubble = self._make_bubble(
            http,
            sent["message_id"],
            message_thread_id=self._topic_thread_id(message),
        )
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
        bubble = self._make_bubble(
            http,
            sent["message_id"],
            message_thread_id=self._topic_thread_id(message),
        )
        await self._handle_restart_action(bubble, thread_id=self._topic_thread_id(message))

    async def _handle_restart_action(
        self, bubble: Bubble, *, thread_id: Optional[int] = None
    ) -> None:
        """Restart behaviour, given an already-sent bubble to drive.

        ``thread_id`` is the topic the user triggered ``/restart`` from;
        the post-restart agent will edit the same bubble in that topic
        (see :meth:`_finalize_restart_marker`). Defaults to the home
        topic when callers don't know it (e.g. /confirm wrappers built
        from arbitrary contexts).
        """
        status = "ok"
        try:
            await self._do_restart_run(bubble, thread_id=thread_id or self.home_topic_id)
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

    async def _do_restart_run(self, bubble: Bubble, *, thread_id: int) -> None:
        # Persist enough info for the post-restart agent to re-attach to
        # this exact bubble. Written *before* spawning pm2 reload so even
        # if pm2 SIGINTs us between the spawn and our own bubble.aclose,
        # the new agent still finds the marker.
        marker = {
            "version": 1,
            "chat_id": self.chat_id,
            "thread_id": int(thread_id),
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

        # Only honour the marker if it's for our supergroup. Any topic
        # within the supergroup is fine -- a /restart can be triggered
        # from the home topic or any other adopted topic, and the
        # post-restart agent must edit the same bubble in that topic.
        if chat_id != self.chat_id:
            logger.warning(
                "restart marker for chat=%s; we are chat=%s; ignoring",
                chat_id, self.chat_id,
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

    async def _handle_abort_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
    ) -> None:
        """Cancel every in-flight handler and ack with a bubble."""
        sent = await self._send_initial_bubble(http, message, text="aborting…")
        if sent is None:
            return
        self._journal_command_start(
            sent["message_id"],
            kind="abort",
            message=message,
            trigger_text="/abort",
        )
        bubble = self._make_bubble(
            http,
            sent["message_id"],
            message_thread_id=self._topic_thread_id(message),
        )
        await self._handle_abort_action(bubble)

    async def _handle_abort_action(self, bubble: Bubble) -> None:
        """Cancel all tasks in ``self._tasks`` except the one running us.

        Cron loops live in ``self._cron_tasks`` and are intentionally
        left alone -- they keep their schedule. The cursor-agent
        subprocess attached to each cancelled handler is torn down by
        ``CursorRunner.run``'s own ``CancelledError`` branch (terminate
        then kill after a 5s grace), so we don't need to await them
        here -- doing so would block ``/abort``'s bubble for up to 5s
        per child for no UX gain.
        """
        status = "ok"
        try:
            me = asyncio.current_task()
            victims = [t for t in self._tasks if t is not me and not t.done()]
            for t in victims:
                t.cancel()
            cleared_confirms = len(self._pending_confirms)
            self._pending_confirms.clear()
            logger.info(
                "/abort cancelled %d handler(s); cleared %d pending confirm(s)",
                len(victims), cleared_confirms,
            )
            await bubble.finalize(
                f"aborted {len(victims)} in-flight handler(s); "
                f"cleared {cleared_confirms} pending confirm(s)"
            )
        except Exception as exc:
            status = "error"
            logger.exception("/abort crashed")
            try:
                await bubble.finalize(f"abort crashed: {exc}")
            except Exception:
                pass
        finally:
            await bubble.aclose()
            self._journal_command_finish(
                bubble.message_id,
                status=status,
                final_text=bubble.last_sent or "",
            )

    async def _handle_plan_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        request: str,
        *,
        reply_context: str = "",
    ) -> None:
        topic_id = self._topic_thread_id(message)
        if not request:
            sent = await self._send_initial_bubble(
                http, message, text="usage: /plan <what you want done>"
            )
            if sent is not None:
                bubble = self._make_bubble(
                    http, sent["message_id"], message_thread_id=topic_id
                )
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

        bubble1 = self._make_bubble(
            http,
            sent1["message_id"],
            message_thread_id=topic_id,
            edit_interval=ROLLOUT_EDIT_INTERVAL,
        )

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
            workdir=self._workdir_for(topic_id),
            model=PLAN_MODEL,
            plan_mode=True,
            system_prompt=self._system_prompt(topic_id),
            extra_prompts=self._extra_prompts(),
        )
        plan_holder: dict[str, str] = {"text": "", "error": ""}
        plan_status = "ok"

        async def _capture_plan_text(text: str) -> None:
            # Capture the worker's raw plan text for phase 2 *before* the
            # supervisor finalises the rollout / produces a recap.
            plan_holder["text"] = text

        try:
            await self._run_supervised(
                http,
                message,
                bubble1,
                plan_runner,
                initial_text=PLAN_INITIAL_BUBBLE_TEXT,
                on_final_capture=_capture_plan_text,
            )
        except asyncio.CancelledError:
            plan_status = "interrupted"
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

        bubble2 = self._make_bubble(
            http,
            sent2["message_id"],
            message_thread_id=topic_id,
            edit_interval=ROLLOUT_EDIT_INTERVAL,
        )

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
            workdir=self._workdir_for(topic_id),
            model=IMPL_MODEL,
            system_prompt=self._system_prompt(topic_id),
            extra_prompts=self._extra_prompts(),
        )
        impl_status = "ok"
        try:
            await self._run_supervised(
                http,
                message,
                bubble2,
                impl_runner,
                initial_text=IMPL_INITIAL_BUBBLE_TEXT,
            )
        except asyncio.CancelledError:
            impl_status = "interrupted"
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
        except Exception:
            impl_status = "error"
            logger.exception("cursor-agent implementation run crashed")
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
                bubble = self._make_bubble(
                    http,
                    sent["message_id"],
                    message_thread_id=self._topic_thread_id(message),
                )
                await bubble.aclose()
            return

        topic_id = self._topic_thread_id(message)
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
            action = lambda b, _tid=topic_id: self._handle_restart_action(b, thread_id=_tid)
        elif self._match_reset_command(wrapped):
            label = "/reset"
            description = "wipe the persistent chat session for this topic"
            action = lambda b, _tid=topic_id: self._handle_reset_action(b, topic_id=_tid)
        else:
            preview = wrapped if len(wrapped) <= 80 else wrapped[:77] + "…"
            label = "prompt"
            description = f"run: {preview}"
            wrapped_prompt = wrapped
            action = lambda b, _p=wrapped_prompt, _http=http, _msg=message: self._run_persistent_chat(
                http=_http, message=_msg, prompt=_p, bubble=b
            )

        await self._prompt_confirmation(
            http,
            message,
            label=label,
            description=description,
            action=action,
        )

    # ---- /cron ----------------------------------------------------------
    async def _handle_cron_command(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        args: str,
    ) -> None:
        """Dispatch ``/cron`` subcommands.

        Forms:
          ``/cron``                  -> list active crons
          ``/cron list``             -> list active crons
          ``/cron rm <id>`` (also ``delete`` / ``remove`` / ``del``)
          ``/cron <mins> <prompt>``  -> schedule
        """
        usage = (
            "usage:\n"
            "/cron <mins> <prompt>   schedule a recurring prompt\n"
            "/cron list              show active crons\n"
            "/cron rm <id>           remove a cron\n"
            "(prompt may span multiple lines)"
        )
        args = args.strip()

        if not args or args.lower() == "list":
            await self._send_short_bubble(http, message, self._render_cron_list())
            return

        # Split on *any* whitespace (space, tab, or newline) so multi-line
        # prompts like `/cron 10\n\nI want you to...` parse correctly --
        # Telegram sends the literal newline, so str.partition(" ") would
        # otherwise treat `10\n<rest>` as a single token.
        parts = args.split(None, 1)
        head = parts[0]
        rest = parts[1] if len(parts) > 1 else ""
        head_lower = head.lower()

        if head_lower in ("rm", "delete", "remove", "del"):
            cid = rest.strip().split(None, 1)[0] if rest.strip() else ""
            if not cid:
                await self._send_short_bubble(http, message, "usage: /cron rm <id>")
                return
            removed = await self._remove_cron(cid)
            text = f"removed cron {cid}" if removed else f"no cron with id {cid}"
            await self._send_short_bubble(http, message, text)
            return

        try:
            mins = int(head)
        except ValueError:
            await self._send_short_bubble(http, message, usage)
            return
        if mins < 1:
            await self._send_short_bubble(http, message, "mins must be a positive integer")
            return
        prompt = rest.strip()
        if not prompt:
            await self._send_short_bubble(http, message, "usage: /cron <mins> <prompt>")
            return

        sender_label = self._trigger_sender_label(message)
        topic_id = self._topic_thread_id(message)
        entry = self._crons.add(
            mins=mins,
            prompt=prompt,
            topic_id=topic_id,
            created_by=sender_label,
        )
        self._spawn_cron_task(entry)
        preview = prompt if len(prompt) <= 200 else prompt[:197] + "…"
        await self._send_short_bubble(
            http,
            message,
            f"scheduled cron {entry.id} every {mins}m in this topic (running now): {preview}",
        )

    async def _send_short_bubble(
        self,
        http: httpx.AsyncClient,
        message: dict[str, Any],
        text: str,
    ) -> None:
        """One-shot reply bubble for command acks (no streaming, no journal)."""
        sent = await self._send_initial_bubble(http, message, text=text)
        if sent is not None:
            bubble = self._make_bubble(
                http,
                sent["message_id"],
                message_thread_id=self._topic_thread_id(message),
            )
            await bubble.aclose()

    def _render_cron_list(self) -> str:
        entries = self._crons.all()
        if not entries:
            return "no active crons.\n/cron <mins> <prompt> to schedule one."
        now = time.time()
        lines = [f"active crons ({len(entries)}):"]
        for e in sorted(entries, key=lambda x: x.id):
            next_at = self._cron_next_at.get(e.id)
            if next_at is None:
                # Either not yet scheduled (just spawned, first run pending)
                # or no task running for this entry (crash / not restored).
                running = e.id in self._cron_tasks
                next_str = "soon" if running else "stopped"
            else:
                next_str = self._fmt_next_in(int(next_at - now))
            preview = e.prompt if len(e.prompt) <= 60 else e.prompt[:57] + "…"
            lines.append(
                f"{e.id}  every {e.mins}m  next in {next_str}  \"{preview}\""
            )
        return "\n".join(lines)

    @staticmethod
    def _fmt_next_in(secs: int) -> str:
        secs = max(0, int(secs))
        if secs < 60:
            return f"{secs}s"
        if secs < 3600:
            return f"{secs // 60}m{secs % 60:02d}s"
        h, rem = divmod(secs, 3600)
        return f"{h}h{rem // 60:02d}m"

    def _spawn_cron_task(self, entry: CronEntry) -> None:
        """Create (or replace) the asyncio loop task for ``entry``."""
        old = self._cron_tasks.pop(entry.id, None)
        if old is not None and not old.done():
            old.cancel()
        # Drop any stale "next in" so /cron list shows "soon" until the
        # first iteration completes and the next sleep is scheduled.
        self._cron_next_at.pop(entry.id, None)
        task = asyncio.create_task(
            self._cron_loop(entry), name=f"cron-{entry.id}"
        )
        self._cron_tasks[entry.id] = task

        def _on_done(t: asyncio.Task[None]) -> None:
            # Remove ourselves only if we're still the registered task --
            # a re-spawn (same id) may have already replaced us.
            if self._cron_tasks.get(entry.id) is t:
                self._cron_tasks.pop(entry.id, None)
                self._cron_next_at.pop(entry.id, None)
            if t.cancelled():
                return
            exc = t.exception()
            if exc is not None:
                logger.error(
                    "cron %s loop crashed: %s", entry.id, exc, exc_info=exc
                )

        task.add_done_callback(_on_done)

    async def _cron_loop(self, entry: CronEntry) -> None:
        """Run ``entry`` immediately, then every ``entry.mins`` minutes."""
        interval = max(1, int(entry.mins)) * 60.0
        try:
            while not self._stop.is_set():
                # Re-fetch from store each tick: if /cron rm landed
                # between iterations, the loop should exit.
                if self._crons.get(entry.id) is None:
                    return
                try:
                    await self._run_cron_iteration(entry)
                except asyncio.CancelledError:
                    raise
                except Exception:
                    logger.exception(
                        "cron %s iteration crashed; will retry next interval",
                        entry.id,
                    )
                self._cron_next_at[entry.id] = time.time() + interval
                try:
                    await asyncio.wait_for(self._stop.wait(), timeout=interval)
                except asyncio.TimeoutError:
                    continue
                # _stop set -> exit cleanly.
                return
        except asyncio.CancelledError:
            raise
        finally:
            self._cron_next_at.pop(entry.id, None)

    async def _run_cron_iteration(self, entry: CronEntry) -> None:
        """Post a fresh bubble into the cron's topic and run ``entry.prompt``
        through that topic's persistent chat session.

        The bubble is *not* a reply (there's no triggering user message);
        the prefix ``(cron <id>)`` makes the origin visually obvious.
        """
        http = self._cron_http
        if http is None:
            logger.warning(
                "cron %s: no http client captured; skipping iteration", entry.id
            )
            return

        topic_id = entry.topic_id or self.home_topic_id
        params: dict[str, Any] = {
            "chat_id": self.chat_id,
            "message_thread_id": topic_id,
            "text": f"(cron {entry.id}) {INITIAL_BUBBLE_TEXT}",
            "disable_web_page_preview": True,
        }
        try:
            resp = await self._api(http, "sendMessage", **params)
        except httpx.HTTPError as exc:
            logger.warning("cron %s sendMessage network error: %s", entry.id, exc)
            return
        if resp.status_code != 200:
            logger.warning(
                "cron %s sendMessage HTTP %d: %s",
                entry.id, resp.status_code, resp.text[:200],
            )
            return
        body = resp.json()
        if not body.get("ok"):
            logger.warning("cron %s sendMessage not ok: %r", entry.id, body)
            return
        sent = body.get("result") or {}
        msg_id = sent.get("message_id")
        if not isinstance(msg_id, int):
            logger.warning("cron %s sendMessage returned no message_id", entry.id)
            return

        bubble = self._make_bubble(
            http,
            msg_id,
            message_thread_id=topic_id,
            edit_interval=ROLLOUT_EDIT_INTERVAL,
        )

        try:
            self._runs.record_start(
                msg_id,
                kind="cron",
                trigger_message_id=None,
                trigger_text=f"/cron {entry.mins} {entry.prompt}",
                trigger_sender=entry.created_by,
                model=CHAT_MODEL,
            )
        except Exception:
            logger.exception("cron %s runjournal record_start crashed", entry.id)

        # _run_persistent_chat needs a message-shaped dict; the only
        # field it actually consumes downstream is to satisfy the
        # supervisor + recap path, which doesn't dereference user info.
        fake_msg: dict[str, Any] = {
            "message_id": msg_id,
            "from": {"username": entry.created_by or f"cron-{entry.id}"},
            "chat": {"id": self.chat_id},
            "message_thread_id": topic_id,
        }

        await self._run_persistent_chat(
            http=http,
            message=fake_msg,
            prompt=entry.prompt,
            bubble=bubble,
        )

    async def _remove_cron(self, cid: str) -> bool:
        """Cancel the in-memory task and drop the persisted entry."""
        existed = self._crons.get(cid) is not None
        task = self._cron_tasks.pop(cid, None)
        self._cron_next_at.pop(cid, None)
        if task is not None and not task.done():
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, Exception):
                pass
        # Always touch the store so a hand-edited journal can still be
        # cleaned up via the command (existed=False -> noop here).
        removed = self._crons.remove(cid)
        return existed or removed

    def _restore_crons(self) -> None:
        """Spawn one asyncio task per persisted cron. Called once at boot.

        Backfills ``topic_id`` onto any legacy entry that was written
        before the multi-topic refactor (when crons were implicitly tied
        to the home topic).
        """
        try:
            self._crons.backfill_topic_id(self.home_topic_id)
            entries = self._crons.all()
        except Exception:
            logger.exception("cron restore: load() crashed; no crons will run")
            return
        if not entries:
            return
        logger.info("restoring %d cron(s) from disk", len(entries))
        for e in entries:
            self._spawn_cron_task(e)

    async def _drain_cron_tasks(self) -> None:
        """Cancel every cron loop and await them. Called on shutdown."""
        if not self._cron_tasks:
            return
        logger.info("cancelling %d cron task(s)…", len(self._cron_tasks))
        tasks = list(self._cron_tasks.values())
        for t in tasks:
            t.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        self._cron_tasks.clear()
        self._cron_next_at.clear()

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

        # Only react to callbacks on bubbles in our own supergroup AND
        # in a topic we're listening on. Random topics in the same
        # supergroup that we never adopted are not ours to act on.
        if (msg.get("chat") or {}).get("id") != self.chat_id:
            await _answer()
            return
        cb_thread_id = msg.get("message_thread_id")
        if not isinstance(cb_thread_id, int) or cb_thread_id not in self._adopted:
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

        bubble = self._make_bubble(
            http, bubble_msg_id, message_thread_id=cb_thread_id
        )

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
            "cursor-agent online: machine=%s chat=%s home_topic=%s adopted=%d starting_offset=%s",
            self.machine, self.chat_id, self.home_topic_id, len(self._adopted), offset,
        )

        async with httpx.AsyncClient(timeout=HTTP_TIMEOUT) as http:
            if offset == 0:
                offset = await self._drain_backlog(http)
                self._save_offset(offset)

            # Boot one outbox watcher per adopted topic. ``_outbox_http``
            # is also consulted by ``_start_outbox_watcher`` so that a
            # topic adopted *at runtime* (via @-mention in a brand-new
            # forum thread) gets its own watcher spawned immediately.
            self._outbox_http = http
            for tid in sorted(self._adopted):
                self._start_outbox_watcher(tid)

            # Ensure every adopted topic's Telegram header reflects the
            # current workspace (set on `/workspace` change, but a fresh
            # boot also has to push the initial render in case the user
            # restarted before any rename was queued, or upgraded onto
            # a build that didn't have this feature).
            for tid in sorted(self._adopted):
                state = topic_state.load(self.paths, tid)
                if state is None:
                    continue
                try:
                    await self._rename_topic_for_workspace(
                        http, tid, state.workspace
                    )
                except Exception:
                    logger.exception(
                        "boot-time topic rename failed (topic=%s)", tid
                    )

            # Capture the agent-lifetime http client so cron loops can
            # post into Telegram without opening their own. Lifetime is
            # bounded by this `async with`, and cron tasks are cancelled
            # in the finally below before the client closes.
            self._cron_http = http
            self._restore_crons()

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
                # Cancel cron loops first so an in-flight iteration
                # doesn't try to use the http client after we close it.
                # They are intentionally NOT journaled in inflight --
                # the persisted entry in crons.json drives recovery on
                # the next boot.
                await self._drain_cron_tasks()
                self._cron_http = None
                # Stop every per-topic outbox watcher.
                for watcher in list(self._outbox_watchers.values()):
                    try:
                        await watcher.stop()
                    except Exception:
                        logger.exception(
                            "outbox watcher stop crashed (topic=%s)",
                            watcher.topic_id,
                        )
                self._outbox_watchers.clear()
                self._outbox_http = None
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
                # SmartRouter is stateless (no httpx client, no
                # subprocess held across calls); nothing to close here.

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
            or self._match_cron_command(text) is not None
            or self._match_workspace_command(text) is not None
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
