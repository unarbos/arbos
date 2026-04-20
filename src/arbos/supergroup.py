"""Create the shared supergroup and ensure it is in forum mode."""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass

from aiotdlib import Client
from aiotdlib.api.types.all import ChatTypeSupergroup

from .errors import FatalInstallError, RecoverableInstallError

logger = logging.getLogger(__name__)


@dataclass
class CreatedSupergroup:
    chat_id: int
    supergroup_id: int
    title: str
    is_forum: bool


def _supergroup_id_from_chat(chat) -> int:
    if not isinstance(chat.type_, ChatTypeSupergroup):
        raise FatalInstallError(
            f"Expected supergroup chat, got {type(chat.type_).__name__}"
        )
    return chat.type_.supergroup_id


def _supergroup_id_from_chat_id(chat_id: int) -> int:
    """Recover the positive supergroup_id from a -100<id> chat_id."""
    # Telegram supergroup chat_ids are encoded as -1000000000000 - supergroup_id.
    if chat_id >= 0:
        raise FatalInstallError(f"chat_id {chat_id} is not a supergroup chat_id")
    sg = -chat_id - 1_000_000_000_000
    if sg <= 0:
        raise FatalInstallError(f"chat_id {chat_id} does not encode a supergroup_id")
    return sg


async def _warm_main_chat_list(client: Client, *, max_pages: int = 10) -> None:
    """Force tdlib to pull the user's main chat list from the server.

    A freshly-authorized session has no local chats and no ``access_hash`` for
    any supergroup, so ``createSupergroupChat(force=False)`` and ``getChat``
    both fail with ``BadRequest: Chat info not found`` -- tdlib refuses to hit
    the network without an access_hash. ``loadChats`` is the documented way to
    page through the main list and populate those access_hashes; it returns a
    404 once everything is loaded.
    """
    for _ in range(max_pages):
        try:
            await client.api.load_chats(limit=200, chat_list=None)
        except Exception as exc:
            msg = str(exc).lower()
            if "404" in msg or "not found" in msg:
                return
            raise


async def adopt_supergroup(
    client: Client,
    *,
    chat_id: int,
    supergroup_id: int | None = None,
) -> CreatedSupergroup:
    """Look up an existing supergroup by chat_id (set by another machine).

    On a freshly-authorized tdlib session the local chat cache is empty, so a
    bare ``getChat`` would return ``BadRequest: Chat not found`` and even
    ``createSupergroupChat(force=False)`` bails with ``Chat info not found``
    because tdlib has no ``access_hash`` to address the peer. We first prime
    the main chat list via ``loadChats`` so the user's supergroups (including
    the one owned by this account) get their access_hashes cached, then
    materialize the chat through ``createSupergroupChat``.

    Used when ``ARBOS_TELEGRAM_CHAT_ID`` was already populated in Doppler by a
    prior machine sharing the same Doppler config; we never want to create a
    second supergroup for the same user.
    """
    if supergroup_id is None:
        supergroup_id = _supergroup_id_from_chat_id(chat_id)

    await _warm_main_chat_list(client)

    # NB: ``force=True`` in TDLib means "create the chat WITHOUT a network
    # request -- info must already be local". That is the cold-cache failure
    # mode we want to avoid. ``force=False`` lets tdlib hit the server when
    # the supergroup row hasn't propagated yet, and surfaces deletion as a
    # real error instead of silently succeeding off stale local state.
    chat = await client.api.create_supergroup_chat(
        supergroup_id=supergroup_id,
        force=False,
    )
    if chat.id != chat_id:
        raise FatalInstallError(
            f"adopted supergroup chat_id {chat.id} does not match expected {chat_id}"
        )
    supergroup = await client.api.get_supergroup(supergroup_id=supergroup_id)
    return CreatedSupergroup(
        chat_id=chat.id,
        supergroup_id=supergroup_id,
        title=chat.title,
        is_forum=bool(supergroup.is_forum),
    )


async def supergroup_exists(
    client: Client,
    *,
    supergroup_id: int,
    chat_id: int | None = None,
) -> bool:
    """Server-side check: is this supergroup still alive and accessible?

    Returns False if the supergroup was deleted via the Telegram UI (or the
    user lost access), True if it can still be opened. Any unexpected error
    is re-raised so transient network failures don't quietly trigger a
    recreation cascade.
    """
    await _warm_main_chat_list(client)
    try:
        chat = await client.api.create_supergroup_chat(
            supergroup_id=supergroup_id,
            force=False,
        )
    except Exception as exc:
        msg = str(exc).lower()
        # tdlib surfaces deletion / lost-access as one of these BadRequests.
        if any(
            tag in msg
            for tag in (
                "chat info not found",
                "chat not found",
                "supergroup not found",
                "channel_private",
                "channel_invalid",
                "user_deactivated",
            )
        ):
            return False
        raise
    if chat_id is not None and chat.id != chat_id:
        return False
    return True


async def create_supergroup(client: Client, *, title: str) -> CreatedSupergroup:
    """Create a forum-enabled supergroup and return its ids."""
    chat = await client.api.create_new_supergroup_chat(
        title=title,
        is_forum=True,
        is_channel=False,
        description="",
        message_auto_delete_time=0,
        for_import=False,
        location=None,
    )

    supergroup_id = _supergroup_id_from_chat(chat)
    supergroup = await client.api.get_supergroup(supergroup_id=supergroup_id)
    return CreatedSupergroup(
        chat_id=chat.id,
        supergroup_id=supergroup_id,
        title=chat.title,
        is_forum=bool(supergroup.is_forum),
    )


async def ensure_forum(
    client: Client,
    *,
    supergroup_id: int,
    poll_interval: float = 1.0,
    timeout: float = 15.0,
) -> bool:
    """Ensure the supergroup is in forum mode; toggle it if not."""
    supergroup = await client.api.get_supergroup(supergroup_id=supergroup_id)
    if supergroup.is_forum:
        return True

    logger.info("supergroup %s is not in forum mode, toggling", supergroup_id)
    await client.api.toggle_supergroup_is_forum(
        supergroup_id=supergroup_id,
        is_forum=True,
    )

    waited = 0.0
    while waited < timeout:
        await asyncio.sleep(poll_interval)
        waited += poll_interval
        supergroup = await client.api.get_supergroup(supergroup_id=supergroup_id)
        if supergroup.is_forum:
            return True

    raise RecoverableInstallError(
        f"Supergroup {supergroup_id} did not flip to forum mode within {timeout:.0f}s"
    )
