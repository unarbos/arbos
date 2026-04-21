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
from .outbox import OutboxWatcher
from .prompts import build_prompt, load_extra_prompts
from . import session_store
from .session_store import ChatSession
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
        self.openai_api_key = os.environ.get("OPENAI_API_KEY", "").strip()
        if not self.openai_api_key:
            logger.warning(
                "OPENAI_API_KEY not set in env — voice transcription will be disabled"
            )
        self._stop = asyncio.Event()
        self._lock_fd: Optional[int] = None
        self._tasks: set[asyncio.Task[None]] = set()

        # Persistent per-topic cursor-agent chat. Loaded from disk so memory
        # survives PM2 restarts. The lock serialises every spawn that touches
        # this chat -- two parallel `--resume <id>` calls would interleave
        # appends.
        loaded = session_store.load(self.paths)
        self._chat_session_id: Optional[str] = loaded.session_id if loaded else None
        self._chat_lock = asyncio.Lock()
        if loaded is not None:
            logger.info("loaded persistent chat session: %s", loaded.session_id)

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
            voice_bubble = Bubble(
                http=http,
                bot_token=self.bot_token,
                chat_id=self.chat_id,
                message_thread_id=self.topic_id,
                message_id=sent["message_id"],
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

        plan_arg = self._match_plan_command(prompt)
        if plan_arg is not None:
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            logger.info("dispatching /plan from %s (len=%d)", sender, len(plan_arg))
            await self._handle_plan_command(http, message, plan_arg)
            return

        if self._match_reset_command(prompt):
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            logger.info("dispatching /reset from %s", sender)
            await self._handle_reset_command(http, message)
            return

        if self._match_update_command(prompt):
            if voice_bubble is not None:
                try:
                    await voice_bubble.finalize(f"voice: {prompt}")
                finally:
                    await voice_bubble.aclose()
            logger.info("dispatching /update from %s", sender)
            await self._handle_update_command(http, message)
            return

        logger.info("dispatching message from %s -> cursor-agent (len=%d)", sender, len(prompt))

        if voice_bubble is not None:
            bubble = voice_bubble
            await bubble.update(INITIAL_BUBBLE_TEXT)
        else:
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

        await self._run_persistent_chat(prompt=prompt, bubble=bubble)

    async def _run_persistent_chat(
        self,
        *,
        prompt: str,
        bubble: Bubble,
    ) -> None:
        """Spawn cursor-agent against the per-topic persistent chat.

        Serialised on ``self._chat_lock`` because cursor-agent's ``--resume``
        appends to a single chat thread; concurrent appends would interleave.
        Auto-recovers from a stale/missing session by clearing the stored id
        and retrying once without ``--resume``.
        """
        async with self._chat_lock:
            try:
                await self._do_persistent_chat_run(prompt=prompt, bubble=bubble)
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

    async def _do_persistent_chat_run(
        self,
        *,
        prompt: str,
        bubble: Bubble,
    ) -> None:
        resume_id = self._chat_session_id

        runner = CursorRunner(
            prompt=prompt,
            workdir=self.workdir,
            model=CHAT_MODEL,
            system_prompt=self._system_prompt(),
            extra_prompts=self._extra_prompts(),
            resume_session=resume_id,
        )
        await runner.run(on_update=bubble.update, on_final=bubble.finalize)

        if runner.resume_failed and resume_id:
            logger.warning(
                "resume of chat %s failed; clearing and starting fresh",
                resume_id,
            )
            self._chat_session_id = None
            session_store.clear(self.paths)
            runner = CursorRunner(
                prompt=prompt,
                workdir=self.workdir,
                model=CHAT_MODEL,
                system_prompt=self._system_prompt(),
                extra_prompts=self._extra_prompts(),
            )
            await runner.run(on_update=bubble.update, on_final=bubble.finalize)

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

        sent = await self._send_initial_bubble(http, message, text=text)
        if sent is None:
            return
        bubble = Bubble(
            http=http,
            bot_token=self.bot_token,
            chat_id=self.chat_id,
            message_thread_id=self.topic_id,
            message_id=sent["message_id"],
        )
        await bubble.aclose()

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
        bubble = Bubble(
            http=http,
            bot_token=self.bot_token,
            chat_id=self.chat_id,
            message_thread_id=self.topic_id,
            message_id=sent["message_id"],
        )

        try:
            await self._do_update_run(bubble)
        except asyncio.CancelledError:
            try:
                await bubble.finalize("(interrupted)")
            finally:
                await bubble.aclose()
            raise
        except Exception as exc:
            logger.exception("/update crashed")
            try:
                await bubble.finalize(f"update crashed: {exc}")
            except Exception:
                pass
            await bubble.aclose()
        else:
            await bubble.aclose()

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
                updater.render_summary(result, machine=self.machine, restarting=False)
            )
            return

        await bubble.finalize(
            updater.render_summary(result, machine=self.machine, restarting=True)
        )

        try:
            updater.spawn_pm2_reload(self.machine)
            logger.info(
                "/update: spawned `pm2 reload arbos-%s`; waiting to be replaced",
                self.machine,
            )
        except UpdateError as exc:
            logger.warning("pm2 reload failed: %s", exc)
            try:
                await bubble.update(
                    updater.render_summary(result, machine=self.machine, restarting=False)
                    + f"\n\npm2 reload failed: {exc}\nrestart manually with `./run.sh restart`"
                )
            except Exception:
                pass

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
            system_prompt=self._system_prompt(),
            extra_prompts=self._extra_prompts(),
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
            system_prompt=self._system_prompt(),
            extra_prompts=self._extra_prompts(),
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
                await self._poll_loop(http, offset)
            finally:
                await outbox.stop()
                await self._drain_tasks()

        logger.info("cursor-agent stopping (signal received)")

    async def _poll_loop(self, http: httpx.AsyncClient, offset: int) -> None:
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
