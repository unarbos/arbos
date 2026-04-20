"""Install-local filesystem layout.

Arbos stores all of its state in ``<install-root>/.arbos/`` -- the install root
is wherever the repo is checked out (``Path.cwd()``). The first ``bootstrap()``
call seeds ``.arbos/.gitignore`` with ``*`` so the entire blob is ignored by
git regardless of the surrounding project's own ``.gitignore``.

Layout::

    <install-root>/                  # cursor-agent workdir
      run.sh
      .arbos/
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

``InstallPaths`` centralises path computation so the rest of the modules never
hard-code joins.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path

_NAME_RE = re.compile(r"[^a-z0-9_]+")
_COLLAPSE_RE = re.compile(r"_+")


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


@dataclass(frozen=True)
class InstallPaths:
    """Resolved paths for a single install root."""

    root: Path

    @classmethod
    def from_path(cls, path: str | os.PathLike[str]) -> "InstallPaths":
        return cls(Path(path).expanduser().resolve())

    @classmethod
    def discover(cls) -> "InstallPaths":
        """Resolve the install root from the current working directory."""
        return cls.from_path(Path.cwd())

    @property
    def arbos(self) -> Path:
        return self.root / ".arbos"

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

    def bootstrap(self) -> None:
        """Create the ``.arbos/`` skeleton with restrictive perms.

        Idempotent. Seeds ``.arbos/.gitignore`` with ``*\\n`` on first run so
        the storage dir is git-ignored even inside a project whose own
        ``.gitignore`` does not list it.
        """
        self.arbos.mkdir(parents=True, exist_ok=True)
        self.secrets.mkdir(parents=True, exist_ok=True)
        self.tdlib.mkdir(parents=True, exist_ok=True)

        for path in (self.arbos, self.secrets, self.tdlib):
            try:
                os.chmod(path, 0o700)
            except OSError:
                pass

        if not self.gitignore_path.exists():
            try:
                self.gitignore_path.write_text("*\n")
            except OSError:
                pass
