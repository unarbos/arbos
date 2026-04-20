"""Drive ``@BotFather`` to create-or-recover a bot's token.

Decision tree for :func:`acquire_bot`:

1. If we already have a token (from Doppler), validate via Bot API ``getMe``.
   If the username matches what we expect, we are done -- no BotFather chat.
2. Otherwise open BotFather and try ``/token``. BotFather replies with a
   list of bots owned by this user. If our desired bot is in that list we
   send ``@<username>`` and parse the returned token.
3. If we don't own the bot yet, drive ``/newbot`` -> display name -> desired
   username (single attempt, no suffixes). On "username taken" we raise a
   fatal error because the per-machine username is deterministic.
"""

from __future__ import annotations

import asyncio
import logging
import re
from dataclasses import dataclass
from typing import Optional

import httpx
from aiotdlib import Client
from aiotdlib.api import API
from aiotdlib.api.types.all import (
    MessageText,
    UpdateNewMessage,
)

from .errors import (
    BotFatherParseError,
    FatalInstallError,
    RecoverableInstallError,
)

logger = logging.getLogger(__name__)

BOTFATHER_USERNAME = "BotFather"
DEFAULT_REPLY_TIMEOUT = 30.0

_TOKEN_RE = re.compile(r"(\d{8,12}:[A-Za-z0-9_-]{30,})")
_TAKEN_HINTS = (
    "username is already taken",
    "username is invalid",
    "sorry, this username is already taken",
)
_NO_BOTS_HINTS = (
    "you don't have any bots yet",
    "you have no bots",
)
_INVALID_BOT_HINTS = (
    "i don't have anything to do",
    "sorry, but i don't have anything to do",
    "invalid bot",
    "invalid username",
)


@dataclass
class CreatedBot:
    username: str
    token: str
    user_id: int


class _BotNotOwned(Exception):
    """Internal sentinel: BotFather doesn't recognise this bot as ours."""


class _ReplyInbox:
    """Async sink that captures BotFather text replies for one chat id."""

    def __init__(self, client: Client, chat_id: int) -> None:
        self._client = client
        self._chat_id = chat_id
        self._queue: asyncio.Queue[str] = asyncio.Queue()
        self._handler = client.add_event_handler(
            self._on_update,
            update_type=API.Types.UPDATE_NEW_MESSAGE,
        )

    async def _on_update(self, _client: Client, update: UpdateNewMessage) -> None:
        message = update.message
        if message.chat_id != self._chat_id:
            return
        if getattr(message, "is_outgoing", False):
            return
        content = message.content
        if isinstance(content, MessageText):
            await self._queue.put(content.text.text)

    async def next(self, timeout: float = DEFAULT_REPLY_TIMEOUT) -> str:
        return await asyncio.wait_for(self._queue.get(), timeout=timeout)

    def close(self) -> None:
        try:
            self._client.remove_event_handler(
                self._handler, update_type=API.Types.UPDATE_NEW_MESSAGE
            )
        except Exception:
            pass


async def _resolve_botfather(client: Client) -> int:
    chat = await client.api.search_public_chat(username=BOTFATHER_USERNAME)
    return chat.id


async def _send_and_await_reply(
    client: Client,
    chat_id: int,
    text: str,
    inbox: _ReplyInbox,
    *,
    timeout: float = DEFAULT_REPLY_TIMEOUT,
) -> str:
    await client.send_text(chat_id, text)
    return await inbox.next(timeout=timeout)


def _parse_token(text: str) -> Optional[str]:
    match = _TOKEN_RE.search(text)
    return match.group(1) if match else None


def _has_hint(reply: str, hints: tuple[str, ...]) -> bool:
    lowered = reply.lower()
    return any(h in lowered for h in hints)


async def _verify_token(token: str) -> dict:
    """Sanity-check a bot token via Bot API ``getMe``; returns the result dict."""
    url = f"https://api.telegram.org/bot{token}/getMe"
    async with httpx.AsyncClient(timeout=15.0) as http:
        resp = await http.get(url)
    if resp.status_code != 200:
        raise BotFatherParseError(
            f"Bot API getMe failed: HTTP {resp.status_code}"
        )
    payload = resp.json()
    if not payload.get("ok"):
        raise BotFatherParseError(f"Bot API getMe returned ok=false: {payload!r}")
    return payload["result"]


async def _try_recover_token(
    client: Client,
    chat_id: int,
    inbox: _ReplyInbox,
    *,
    desired_username: str,
) -> str:
    """Drive ``/token`` -> ``@username`` to retrieve an existing bot's token.

    Raises :class:`_BotNotOwned` if BotFather does not list ``desired_username``
    among the user's bots.
    """
    list_reply = await _send_and_await_reply(client, chat_id, "/token", inbox)

    if _has_hint(list_reply, _NO_BOTS_HINTS):
        raise _BotNotOwned()

    handle = "@" + desired_username
    if handle.lower() not in list_reply.lower():
        # BotFather replies with the inline-keyboard list as text; if our
        # target isn't there, it isn't ours.
        # Cancel any pending interactive flow on BotFather's side.
        try:
            await _send_and_await_reply(client, chat_id, "/cancel", inbox, timeout=10.0)
        except asyncio.TimeoutError:
            pass
        raise _BotNotOwned()

    token_reply = await _send_and_await_reply(client, chat_id, handle, inbox)
    if _has_hint(token_reply, _INVALID_BOT_HINTS):
        raise _BotNotOwned()

    token = _parse_token(token_reply)
    if not token:
        raise BotFatherParseError(
            f"BotFather did not return a token for {handle!r}; reply was:\n{token_reply}"
        )
    return token


async def _create_via_newbot(
    client: Client,
    chat_id: int,
    inbox: _ReplyInbox,
    *,
    display_name: str,
    desired_username: str,
) -> str:
    """Drive ``/newbot`` -> name -> username with a single deterministic try."""
    await _send_and_await_reply(client, chat_id, "/newbot", inbox)
    await _send_and_await_reply(client, chat_id, display_name, inbox)
    reply = await _send_and_await_reply(client, chat_id, desired_username, inbox)

    token = _parse_token(reply)
    if token:
        return token

    if _has_hint(reply, _TAKEN_HINTS):
        raise FatalInstallError(
            f"@{desired_username} is taken by someone else; pick a different "
            f"machine name or release the existing bot. BotFather said:\n{reply}"
        )

    raise BotFatherParseError(
        f"Unexpected BotFather reply while creating @{desired_username}:\n{reply}"
    )


async def disable_privacy(client: Client, *, bot_username: str) -> str:
    """Drive BotFather ``/setprivacy`` to OFF so the bot can read all group messages.

    Idempotent: BotFather happily reports the current state if it's already
    disabled, and we treat that as success.
    """
    bot_username = bot_username.lstrip("@")
    chat_id = await _resolve_botfather(client)
    inbox = _ReplyInbox(client, chat_id)
    try:
        list_reply = await _send_and_await_reply(client, chat_id, "/setprivacy", inbox)
        if _has_hint(list_reply, _NO_BOTS_HINTS):
            raise FatalInstallError(
                "BotFather says you have no bots; cannot toggle privacy"
            )
        reply = await _send_and_await_reply(
            client, chat_id, "@" + bot_username, inbox
        )
        # If already disabled, BotFather may surface that in this same reply.
        lowered = reply.lower()
        if "current status is: disabled" in lowered or "is currently disabled" in lowered:
            logger.info("privacy mode already disabled for @%s", bot_username)
            try:
                await _send_and_await_reply(client, chat_id, "/cancel", inbox, timeout=10.0)
            except asyncio.TimeoutError:
                pass
            return reply

        confirm = await _send_and_await_reply(client, chat_id, "Disable", inbox)
        if "disabled" not in confirm.lower():
            raise BotFatherParseError(
                f"BotFather did not confirm privacy disable; reply was:\n{confirm}"
            )
        logger.info("privacy mode disabled for @%s", bot_username)
        return confirm
    except asyncio.TimeoutError as exc:
        raise RecoverableInstallError(
            "Timed out talking to BotFather for /setprivacy; try again"
        ) from exc
    finally:
        inbox.close()


async def acquire_bot(
    client: Client,
    *,
    desired_username: str,
    display_name: str,
    existing_token: Optional[str] = None,
) -> CreatedBot:
    """Return a :class:`CreatedBot` for ``desired_username``.

    Tries, in order: existing token validation -> ``/token`` recovery
    -> ``/newbot`` creation.
    """
    desired_username = desired_username.lstrip("@")

    if existing_token:
        try:
            result = await _verify_token(existing_token)
            actual = (result.get("username") or "").lower()
            if actual == desired_username.lower():
                logger.info("reusing existing bot token for @%s", desired_username)
                return CreatedBot(
                    username=desired_username,
                    token=existing_token,
                    user_id=int(result["id"]),
                )
            logger.warning(
                "existing token belongs to @%s but expected @%s; ignoring it",
                actual,
                desired_username,
            )
        except BotFatherParseError as exc:
            logger.warning("existing token rejected by getMe (%s); falling back to BotFather", exc)

    chat_id = await _resolve_botfather(client)
    inbox = _ReplyInbox(client, chat_id)
    try:
        try:
            token = await _try_recover_token(
                client, chat_id, inbox, desired_username=desired_username
            )
            logger.info("recovered existing bot token for @%s via /token", desired_username)
        except _BotNotOwned:
            logger.info("no existing bot @%s; running /newbot", desired_username)
            token = await _create_via_newbot(
                client,
                chat_id,
                inbox,
                display_name=display_name,
                desired_username=desired_username,
            )

        result = await _verify_token(token)
        return CreatedBot(
            username=desired_username,
            token=token,
            user_id=int(result["id"]),
        )
    except asyncio.TimeoutError as exc:
        raise RecoverableInstallError(
            "Timed out waiting for a BotFather reply; try `./run.sh` again"
        ) from exc
    finally:
        inbox.close()
