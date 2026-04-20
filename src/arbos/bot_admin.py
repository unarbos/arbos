"""Add the bot to the shared supergroup and promote it to admin.

The granted rights are deliberately minimal: enough for the runtime to manage
forum topics and post/delete its own messages, but no member moderation,
no info changes, no promotion of others.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass

from aiotdlib import Client
from aiotdlib.api.types.all import (
    ChatAdministratorRights,
    ChatMemberStatusAdministrator,
    ChatMemberStatusBanned,
    ChatMemberStatusLeft,
    ChatMemberStatusMember,
    ChatMemberStatusRestricted,
    MessageSenderUser,
)

from .errors import FatalInstallError

logger = logging.getLogger(__name__)


@dataclass
class BotMembership:
    bot_user_id: int
    chat_id: int


async def resolve_bot_user_id(client: Client, *, bot_username: str) -> int:
    """Look up the bot's numeric user id by its public username."""
    chat = await client.api.search_public_chat(username=bot_username.lstrip("@"))
    user_id = chat.id
    user = await client.get_user(user_id)
    if not user.type_.ID.endswith("Bot"):
        raise FatalInstallError(
            f"Resolved @{bot_username} but it is not a bot account (type={user.type_.ID})"
        )
    return user.id


def _admin_rights() -> ChatAdministratorRights:
    return ChatAdministratorRights(
        can_manage_chat=True,
        can_change_info=False,
        can_post_messages=False,
        can_edit_messages=False,
        can_delete_messages=True,
        can_invite_users=True,
        can_restrict_members=False,
        can_pin_messages=False,
        can_manage_topics=True,
        can_promote_members=False,
        can_manage_video_chats=False,
        can_post_stories=False,
        can_edit_stories=False,
        can_delete_stories=False,
        is_anonymous=False,
    )


async def _current_status(client: Client, *, chat_id: int, bot_user_id: int):
    """Return the bot's current ``ChatMemberStatus*`` in this chat, or None."""
    try:
        member = await client.api.get_chat_member(
            chat_id=chat_id,
            member_id=MessageSenderUser(user_id=bot_user_id),
        )
    except Exception:
        return None
    return member.status


async def add_and_promote(
    client: Client,
    *,
    chat_id: int,
    bot_username: str,
) -> BotMembership:
    """Add the bot to ``chat_id`` and promote it to admin (idempotent).

    On reruns where the bot is already an admin, both ``add_chat_member`` and
    ``set_chat_member_status`` are skipped. Otherwise we add (when
    ``Left``/``Banned``/missing) and always normalise admin rights.
    """
    bot_user_id = await resolve_bot_user_id(client, bot_username=bot_username)
    logger.info("resolved bot @%s -> user_id=%s", bot_username, bot_user_id)

    status = await _current_status(client, chat_id=chat_id, bot_user_id=bot_user_id)

    if isinstance(status, ChatMemberStatusAdministrator):
        logger.info("bot @%s already admin in chat %s; nothing to do", bot_username, chat_id)
        return BotMembership(bot_user_id=bot_user_id, chat_id=chat_id)

    needs_add = status is None or isinstance(
        status, (ChatMemberStatusLeft, ChatMemberStatusBanned)
    )

    if needs_add:
        failed = await client.api.add_chat_member(
            chat_id=chat_id,
            user_id=bot_user_id,
            forward_limit=0,
        )
        if failed and getattr(failed, "failed_to_add_members", []):
            members = failed.failed_to_add_members
            if members:
                raise FatalInstallError(
                    f"Failed to add bot to supergroup: {members[0]!r}"
                )

    await client.api.set_chat_member_status(
        chat_id=chat_id,
        member_id=MessageSenderUser(user_id=bot_user_id),
        status=ChatMemberStatusAdministrator(
            custom_title="agent",
            can_be_edited=True,
            rights=_admin_rights(),
        ),
    )

    return BotMembership(bot_user_id=bot_user_id, chat_id=chat_id)
