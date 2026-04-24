"""File-drop outbox: any file landed in a topic's ``outbox/`` is posted to
that topic and then moved aside.

Why a filesystem queue rather than a richer IPC channel?

The cursor-agent that handles each Telegram message runs as a *child* of the
long-lived agent process; there is no in-process call path back from a child
back to ``CursorAgent``. A drop-zone per adopted topic at
``~/.arbos/topics/<topic_id>/outbox/`` is the simplest thing that gives
every child (and every other process on the host) a way to post images,
PDFs, traces, screenshots, etc. into the right topic without growing a
control-plane.

The agent owns one :class:`OutboxWatcher` per adopted topic. New topics
adopted at runtime get a fresh watcher spun up by the agent layer.

Behaviour
---------
Polled once per second. For each *stable* file at the top level of
``outbox_dir``:

* Filename extension picks the Telegram method: images -> ``sendPhoto``,
  ``.gif`` -> ``sendAnimation``, audio -> ``sendAudio``, voice notes ->
  ``sendVoice``, video -> ``sendVideo``, anything else -> ``sendDocument``.
* Optional caption comes from a sibling ``<basename>.caption.txt`` (read,
  truncated to Telegram's 1024-char limit, attached, then moved along).
* On success the file (and its caption sidecar) is moved to
  ``outbox/sent/`` with a UTC timestamp prefix to avoid collisions.
* On failure it is moved to ``outbox/failed/`` with a sibling
  ``<name>.error.txt`` describing the failure -- the watcher will not retry,
  so transient errors should be retried by re-dropping the file.

Stability
---------
We only post a file once its ``(size, mtime_ns)`` tuple has been observed
unchanged for one polling tick. This catches the common case where a writer
streams content directly into the outbox; callers that want to be perfectly
safe should write to ``<name>.partial`` (ignored) and atomically
``os.replace`` to the final name when done.

The watcher deliberately ignores:

* hidden dotfiles (``.``-prefixed)
* ``*.partial``, ``*.tmp`` (in-progress writes)
* ``*.caption.txt``, ``*.error.txt`` (sidecars consumed alongside their main file)
* anything inside ``sent/`` or ``failed/``
"""

from __future__ import annotations

import asyncio
import logging
import mimetypes
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

import httpx

from .workspace import InstallPaths

logger = logging.getLogger(__name__)


BOT_API = "https://api.telegram.org"

# Outbox poll cadence. 1s feels human-instant on Telegram while keeping the
# load near zero (a single readdir on a tiny dir).
POLL_INTERVAL = 1.0

# Telegram's hard caption limit for sendPhoto/Document/etc. is 1024 chars.
MAX_CAPTION = 1024

# Telegram Bot API hard upload limits.
#   - sendPhoto:    10 MB
#   - everything else (Document/Video/Audio/Animation/Voice): 50 MB
# We check up front so callers get a fast, descriptive failure instead of
# the truncated server-side error.
MAX_PHOTO_BYTES = 10 * 1024 * 1024
MAX_OTHER_BYTES = 50 * 1024 * 1024

# Per-request HTTP budget for media uploads. Generous because a 50 MB
# upload over a slow link easily takes tens of seconds.
UPLOAD_TIMEOUT = 120.0

CAPTION_SUFFIX = ".caption.txt"
ERROR_SUFFIX = ".error.txt"
IGNORED_SUFFIXES = (".partial", ".tmp", CAPTION_SUFFIX, ERROR_SUFFIX)

# Extension -> (Telegram method, form field name). Matches what the Bot API
# expects for the multipart upload.
_PHOTO_EXTS = {".jpg", ".jpeg", ".png", ".webp"}
_VIDEO_EXTS = {".mp4", ".mov", ".m4v"}
_AUDIO_EXTS = {".mp3", ".m4a", ".flac", ".wav"}
_VOICE_EXTS = {".ogg", ".oga", ".opus"}
_GIF_EXTS = {".gif"}


def _classify(path: Path) -> tuple[str, str]:
    """Return ``(method, field_name)`` for ``path``'s extension.

    Falls back to ``sendDocument`` for any unrecognised extension so the
    outbox handles arbitrary file types without surprise.
    """
    ext = path.suffix.lower()
    if ext in _PHOTO_EXTS:
        return ("sendPhoto", "photo")
    if ext in _GIF_EXTS:
        return ("sendAnimation", "animation")
    if ext in _VIDEO_EXTS:
        return ("sendVideo", "video")
    if ext in _VOICE_EXTS:
        return ("sendVoice", "voice")
    if ext in _AUDIO_EXTS:
        return ("sendAudio", "audio")
    return ("sendDocument", "document")


def _is_eligible(path: Path) -> bool:
    """True iff ``path`` is a regular file the watcher should consider."""
    name = path.name
    if name.startswith("."):
        return False
    for sfx in IGNORED_SUFFIXES:
        if name.endswith(sfx):
            return False
    try:
        return path.is_file()
    except OSError:
        return False


def _stat_key(path: Path) -> Optional[tuple[int, int]]:
    """``(size, mtime_ns)`` snapshot, or ``None`` if the file vanished."""
    try:
        st = path.stat()
    except OSError:
        return None
    return (st.st_size, st.st_mtime_ns)


def _read_caption(media_path: Path) -> tuple[str, Optional[Path]]:
    """Return ``(caption, sidecar_path)``; both may be empty/None.

    Sidecar lookup tries, in order:
      1. ``<full-name>.caption.txt``     -- e.g. ``foo.png.caption.txt``
      2. ``<stem>.caption.txt``          -- e.g. ``foo.caption.txt``

    The sidecar path is returned so the caller can move it alongside the
    main file (so ``sent/`` is self-contained).
    """
    candidates = [
        media_path.with_name(media_path.name + CAPTION_SUFFIX),
        media_path.with_suffix(CAPTION_SUFFIX),
    ]
    for cap in candidates:
        if cap == media_path:
            continue
        if not cap.is_file():
            continue
        try:
            text = cap.read_text(encoding="utf-8", errors="replace")
        except OSError as exc:
            logger.warning("caption %s unreadable: %s", cap, exc)
            return ("", cap)
        text = text.strip()
        if len(text) > MAX_CAPTION:
            text = text[: MAX_CAPTION - 1] + "…"
        return (text, cap)
    return ("", None)


def _ts_prefix() -> str:
    """``20260421T071530Z-`` style prefix; sortable + collision-resistant."""
    return datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ-")


def _move_aside(path: Path, dest_dir: Path) -> Path:
    """Move ``path`` into ``dest_dir`` with a timestamp prefix, returning the new path.

    Uses ``os.replace`` so the move is atomic on the same filesystem; if it
    fails we log and leave the file in place (better to potentially re-post
    on next tick than silently lose it).
    """
    dest_dir.mkdir(parents=True, exist_ok=True)
    target = dest_dir / (_ts_prefix() + path.name)
    # Defensive: if a same-second collision occurs, append a counter.
    if target.exists():
        i = 1
        while True:
            alt = dest_dir / f"{target.stem}.{i}{target.suffix}"
            if not alt.exists():
                target = alt
                break
            i += 1
    try:
        os.replace(path, target)
    except OSError as exc:
        logger.warning("could not move %s -> %s: %s", path, target, exc)
        return path
    return target


class OutboxWatcher:
    """Background task that ships files from a per-topic outbox to Telegram."""

    def __init__(
        self,
        *,
        http: httpx.AsyncClient,
        bot_token: str,
        chat_id: int,
        message_thread_id: int,
        paths: InstallPaths,
        topic_id: int,
        poll_interval: float = POLL_INTERVAL,
    ) -> None:
        self._http = http
        self._bot_token = bot_token
        self._chat_id = chat_id
        self._thread_id = message_thread_id
        self._paths = paths
        self._topic_id = int(topic_id)
        self._outbox_dir = paths.topic_outbox_dir(self._topic_id)
        self._sent_dir = paths.topic_outbox_sent_dir(self._topic_id)
        self._failed_dir = paths.topic_outbox_failed_dir(self._topic_id)
        self._poll_interval = poll_interval
        self._stop = asyncio.Event()
        self._task: Optional[asyncio.Task[None]] = None
        # path -> last-seen (size, mtime_ns); used to require one tick of
        # stability before posting, which catches partial writes.
        self._seen: dict[Path, tuple[int, int]] = {}

    @property
    def topic_id(self) -> int:
        return self._topic_id

    def start(self) -> None:
        if self._task is not None:
            return
        self._paths.bootstrap_topic(self._topic_id)
        self._task = asyncio.create_task(
            self._loop(), name=f"outbox-watcher-{self._topic_id}"
        )
        logger.info(
            "outbox watcher started: topic=%s dir=%s every=%.1fs",
            self._topic_id, self._outbox_dir, self._poll_interval,
        )

    async def stop(self) -> None:
        self._stop.set()
        if self._task is None:
            return
        self._task.cancel()
        try:
            await self._task
        except (asyncio.CancelledError, Exception):
            pass
        self._task = None

    async def _loop(self) -> None:
        try:
            while not self._stop.is_set():
                try:
                    await self._tick()
                except Exception:
                    logger.exception("outbox tick crashed; continuing")
                try:
                    await asyncio.wait_for(
                        self._stop.wait(), timeout=self._poll_interval
                    )
                    return
                except asyncio.TimeoutError:
                    pass
        except asyncio.CancelledError:
            raise

    async def _tick(self) -> None:
        outbox = self._outbox_dir
        try:
            entries = sorted(outbox.iterdir(), key=lambda p: p.name)
        except FileNotFoundError:
            return
        except OSError as exc:
            logger.warning("outbox scan failed: %s", exc)
            return

        eligible: list[Path] = [p for p in entries if _is_eligible(p)]

        # Drop stale stability cache entries (file deleted / moved).
        live = {p for p in eligible}
        for p in list(self._seen):
            if p not in live:
                self._seen.pop(p, None)

        for path in eligible:
            key = _stat_key(path)
            if key is None:
                self._seen.pop(path, None)
                continue
            prev = self._seen.get(path)
            if prev != key:
                # Not yet stable; remember this snapshot and try again next tick.
                self._seen[path] = key
                continue

            # Stable -- post it.
            self._seen.pop(path, None)
            await self._post(path)

    async def _post(self, path: Path) -> None:
        method, field = _classify(path)
        try:
            size = path.stat().st_size
        except OSError as exc:
            logger.warning("outbox: %s vanished before send: %s", path, exc)
            return

        if method == "sendPhoto" and size > MAX_PHOTO_BYTES:
            self._fail(
                path,
                None,
                f"photo too large ({size} bytes > {MAX_PHOTO_BYTES}); "
                "drop it as a generic file (e.g. .bin) to send via sendDocument",
            )
            return
        if method != "sendPhoto" and size > MAX_OTHER_BYTES:
            self._fail(
                path,
                None,
                f"file too large ({size} bytes > {MAX_OTHER_BYTES}); "
                "Telegram bot upload cap is 50 MB",
            )
            return

        caption, caption_path = _read_caption(path)

        try:
            data = path.read_bytes()
        except OSError as exc:
            self._fail(path, caption_path, f"read failed: {exc}")
            return

        mime, _ = mimetypes.guess_type(path.name)
        files = {field: (path.name, data, mime or "application/octet-stream")}
        form: dict[str, str] = {
            "chat_id": str(self._chat_id),
            "message_thread_id": str(self._thread_id),
        }
        if caption:
            form["caption"] = caption

        url = f"{BOT_API}/bot{self._bot_token}/{method}"
        try:
            resp = await self._http.post(
                url, data=form, files=files, timeout=UPLOAD_TIMEOUT
            )
        except httpx.HTTPError as exc:
            self._fail(path, caption_path, f"network error: {exc}")
            return

        if resp.status_code == 429:
            try:
                retry_after = float(
                    resp.json().get("parameters", {}).get("retry_after", 1.0)
                )
            except Exception:
                retry_after = 1.0
            logger.info(
                "outbox %s 429; sleeping %.2fs and re-queuing %s",
                method, retry_after, path.name,
            )
            await asyncio.sleep(retry_after)
            # Don't move the file; next tick will try again from scratch.
            return

        if resp.status_code != 200:
            self._fail(
                path,
                caption_path,
                f"{method} HTTP {resp.status_code}: {resp.text[:300]}",
            )
            return

        body = resp.json()
        if not body.get("ok"):
            self._fail(path, caption_path, f"{method} not ok: {body!r}")
            return

        sent_path = _move_aside(path, self._sent_dir)
        if caption_path is not None and caption_path.exists():
            _move_aside(caption_path, self._sent_dir)
        logger.info(
            "outbox %s OK: %s (%d bytes)%s",
            method,
            sent_path.name,
            size,
            f" caption={caption[:40]!r}" if caption else "",
        )

    def _fail(
        self, path: Path, caption_path: Optional[Path], reason: str
    ) -> None:
        logger.warning("outbox failed %s: %s", path.name, reason)
        moved = _move_aside(path, self._failed_dir)
        if caption_path is not None and caption_path.exists():
            _move_aside(caption_path, self._failed_dir)
        try:
            moved.with_name(moved.name + ERROR_SUFFIX).write_text(
                reason + "\n", encoding="utf-8"
            )
        except OSError as exc:
            logger.warning(
                "could not write error sidecar for %s: %s", moved, exc
            )
