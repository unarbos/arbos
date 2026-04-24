"""Filesystem layout for arbos.

Arbos stores all of its state under a single, fixed location per machine:
``~/.arbos/``. There is exactly one install per user account on the host.
The cursor-agent **workdir** is decoupled from the storage location and
becomes a per-topic setting (see ``topic_state.py``); ``./run.sh`` can be
invoked from anywhere and always produces the same install.

Layout::

    ~/.arbos/
      .gitignore                   # contains "*\n"
      config.json
      install_state.json
      install.log
      agent.log
      agent.offset
      agent.lock
      doppler_writeback.json
      secrets/                     # 0700, bot_token.txt, api_hash.txt
      tdlib/                       # aiotdlib files_directory
      prompts/                     # PROMPT.md + extras
      runs/                        # per-Telegram-message-id run journal
      crons.json                   # recurring prompts
      inflight.json                # crash-recovery journal
      restart_marker.json          # /restart bubble pointer
      src/                         # arbos source clone (run.sh bootstrap)
      topics/<topic_id>/
        state.json                 # workspace path, adopted_at, ...
        chat_session.json          # per-topic persistent cursor-agent chat
        router_session.json        # per-topic persistent SmartRouter chat
        outbox/                    # per-topic file-drop outbox

The ``ARBOS_HOME`` environment variable overrides the storage location
(intended for tests). When unset, ``~/.arbos/`` is used unconditionally.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path

_NAME_RE = re.compile(r"[^a-z0-9_]+")
_COLLAPSE_RE = re.compile(r"_+")

ARBOS_HOME_ENV = "ARBOS_HOME"


def normalize_project_name(raw: str) -> str:
    """Lower-case, strip, replace any non ``[a-z0-9_]`` run with ``_``.

    The same normalisation is applied to forum-topic titles, and machine-name
    suffixes used in bot usernames -- which Telegram restricts to
    ``[A-Za-z0-9_]`` only (no hyphens).
    """
    name = raw.strip().lower()
    name = _NAME_RE.sub("_", name)
    name = _COLLAPSE_RE.sub("_", name).strip("_")
    if not name:
        raise ValueError(f"project name {raw!r} is empty after normalisation")
    return name


def _default_arbos_home() -> Path:
    """Resolve the ``~/.arbos`` storage dir, honouring ``ARBOS_HOME``."""
    override = os.environ.get(ARBOS_HOME_ENV, "").strip()
    if override:
        return Path(override).expanduser().resolve()
    return Path.home() / ".arbos"


@dataclass(frozen=True)
class InstallPaths:
    """Resolved paths for the single per-host install."""

    arbos: Path

    @classmethod
    def from_path(cls, path: str | os.PathLike[str]) -> "InstallPaths":
        return cls(Path(path).expanduser().resolve())

    @classmethod
    def discover(cls) -> "InstallPaths":
        """Resolve the canonical ``~/.arbos`` storage dir."""
        return cls(_default_arbos_home())

    # NOTE: ``root`` is retained as an alias of ``arbos`` so a small number
    # of legacy call sites (logging, an ``install_root`` field in
    # ``StoredConfig``) keep working without churn. Anything that previously
    # used ``paths.root`` as a *cursor-agent workdir* must now resolve the
    # workdir via ``topic_state.workspace_for(...)`` instead -- the
    # install location is no longer a workspace.
    @property
    def root(self) -> Path:
        return self.arbos

    @property
    def secrets(self) -> Path:
        return self.arbos / "secrets"

    @property
    def tdlib(self) -> Path:
        return self.arbos / "tdlib"

    @property
    def config_path(self) -> Path:
        return self.arbos / "config.json"

    @property
    def state_path(self) -> Path:
        return self.arbos / "install_state.json"

    @property
    def log_path(self) -> Path:
        return self.arbos / "install.log"

    @property
    def agent_log_path(self) -> Path:
        return self.arbos / "agent.log"

    @property
    def offset_path(self) -> Path:
        return self.arbos / "agent.offset"

    @property
    def lock_path(self) -> Path:
        return self.arbos / "agent.lock"

    @property
    def bot_token_path(self) -> Path:
        return self.secrets / "bot_token.txt"

    @property
    def api_hash_path(self) -> Path:
        return self.secrets / "api_hash.txt"

    @property
    def writeback_path(self) -> Path:
        return self.arbos / "doppler_writeback.json"

    @property
    def gitignore_path(self) -> Path:
        return self.arbos / ".gitignore"

    @property
    def prompts_dir(self) -> Path:
        return self.arbos / "prompts"

    @property
    def prompt_md_path(self) -> Path:
        return self.prompts_dir / "PROMPT.md"

    @property
    def src_dir(self) -> Path:
        """Where ``run.sh`` clones the arbos repo when bootstrapping."""
        return self.arbos / "src"

    @property
    def inflight_path(self) -> Path:
        """Crash-recovery journal of Telegram updates currently being handled.

        See ``inflight.py`` for the schema. The file is written atomically
        on every state transition (``pending`` -> ``done``/``failed``/
        ``interrupted``) so a crash mid-handler never loses the record
        of what we owed the user.
        """
        return self.arbos / "inflight.json"

    @property
    def restart_marker_path(self) -> Path:
        """Pre-restart bubble pointer for ``/restart``.

        Written immediately before we spawn ``pm2 reload`` so the freshly
        booted agent can edit the *same* bubble in-place to a "back online"
        message instead of posting a new one. Deleted by the new agent
        once it has finalised the bubble.
        """
        return self.arbos / "restart_marker.json"

    @property
    def crons_path(self) -> Path:
        """Persistent recurring-prompt schedules for this install.

        See ``crons.py``. One JSON document holding every active cron
        plus the ``next_id`` counter. Reloaded on every agent boot so
        crons survive PM2 restarts and ``/update``.
        """
        return self.arbos / "crons.json"

    @property
    def runs_dir(self) -> Path:
        """Per-bubble run journal entries, one JSON file per Telegram message_id.

        See ``runjournal.py``. Used to give the agent grounded context when
        the user replies to a previous Arbos bubble.
        """
        return self.arbos / "runs"

    # ---- per-topic paths -------------------------------------------------

    @property
    def topics_dir(self) -> Path:
        """Parent dir of every adopted topic's per-topic state."""
        return self.arbos / "topics"

    def topic_dir(self, topic_id: int) -> Path:
        """Per-topic state directory: workspace, chat session, outbox."""
        return self.topics_dir / str(int(topic_id))

    def topic_state_path(self, topic_id: int) -> Path:
        """Per-topic ``state.json`` (workspace, adopted_at, last_seen_at)."""
        return self.topic_dir(topic_id) / "state.json"

    def topic_chat_session_path(self, topic_id: int) -> Path:
        """Per-topic persistent cursor-agent chat session id."""
        return self.topic_dir(topic_id) / "chat_session.json"

    def topic_router_session_path(self, topic_id: int) -> Path:
        """Per-topic persistent cursor-agent SmartRouter session id.

        Separate from the worker's chat session so routing-decision
        thinking ('what should I do with this message?' → ``/cron`` /
        ``/delegate``) doesn't pollute the worker's coding chat history,
        and vice-versa. Both threads still independently see every user
        message that arrives in this topic.
        """
        return self.topic_dir(topic_id) / "router_session.json"

    def topic_outbox_dir(self, topic_id: int) -> Path:
        """Per-topic file-drop outbox watched by ``OutboxWatcher``."""
        return self.topic_dir(topic_id) / "outbox"

    def topic_outbox_sent_dir(self, topic_id: int) -> Path:
        return self.topic_outbox_dir(topic_id) / "sent"

    def topic_outbox_failed_dir(self, topic_id: int) -> Path:
        return self.topic_outbox_dir(topic_id) / "failed"

    def bootstrap(self) -> None:
        """Create the ``~/.arbos/`` skeleton with restrictive perms.

        Idempotent. Seeds ``.arbos/.gitignore`` with ``*\\n`` on first run so
        the storage dir is git-ignored even inside a project whose own
        ``.gitignore`` does not list it.
        """
        self.arbos.mkdir(parents=True, exist_ok=True)
        self.secrets.mkdir(parents=True, exist_ok=True)
        self.tdlib.mkdir(parents=True, exist_ok=True)
        self.prompts_dir.mkdir(parents=True, exist_ok=True)
        self.topics_dir.mkdir(parents=True, exist_ok=True)
        self.runs_dir.mkdir(parents=True, exist_ok=True)

        for path in (
            self.arbos,
            self.secrets,
            self.tdlib,
            self.prompts_dir,
            self.topics_dir,
            self.runs_dir,
        ):
            try:
                os.chmod(path, 0o700)
            except OSError:
                pass

        if not self.gitignore_path.exists():
            try:
                self.gitignore_path.write_text("*\n")
            except OSError:
                pass

    def bootstrap_topic(self, topic_id: int) -> None:
        """Create the per-topic state subtree (idempotent)."""
        topic_id = int(topic_id)
        for path in (
            self.topic_dir(topic_id),
            self.topic_outbox_dir(topic_id),
            self.topic_outbox_sent_dir(topic_id),
            self.topic_outbox_failed_dir(topic_id),
        ):
            path.mkdir(parents=True, exist_ok=True)
            try:
                os.chmod(path, 0o700)
            except OSError:
                pass
