"""Per-machine forum topic creation and welcome-message seeding.

Each machine running ``./run.sh`` creates exactly one forum topic in the
shared supergroup, named after the machine. Re-runs are guarded by the
install state machine; this module itself is unconditional.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass

from aiotdlib import Client
from aiotdlib.api.types.all import (
    FormattedText,
    ForumTopicIcon,
    InputMessageText,
)

from .workspace import normalize_project_name

logger = logging.getLogger(__name__)

_TOPIC_ICON_COLORS: tuple[int, ...] = (
    0x6FB9F0,
    0xFFD67E,
    0xCB86DB,
    0x8EEE98,
    0xFF93B2,
    0xFB6F5F,
)


@dataclass
class CreatedTopic:
    name: str
    topic_id: int
    welcome_message_id: int | None = None
    is_new: bool = False


def _icon_for(name: str) -> ForumTopicIcon:
    color = _TOPIC_ICON_COLORS[abs(hash(name)) % len(_TOPIC_ICON_COLORS)]
    return ForumTopicIcon(color=color, custom_emoji_id=0)


def _welcome_text(name: str) -> str:
    return (
        f"Machine: {name}\n\n"
        f"Use this thread for all future interaction with this machine."
    )


async def topic_exists(
    client: Client,
    *,
    chat_id: int,
    topic_id: int,
) -> bool:
    """True if the given forum topic still exists in the supergroup."""
    try:
        await client.api.get_forum_topic(
            chat_id=chat_id,
            message_thread_id=topic_id,
        )
        return True
    except Exception as exc:
        msg = str(exc).lower()
        if any(
            tag in msg
            for tag in (
                "topic not found",
                "message thread not found",
                "thread not found",
                "chat not found",
                "message not found",
            )
        ):
            return False
        raise


async def find_topic_by_name(
    client: Client,
    *,
    chat_id: int,
    name: str,
) -> int | None:
    """Search the supergroup's forum for a topic whose name matches exactly."""
    try:
        result = await client.api.get_forum_topics(
            chat_id=chat_id,
            query=name,
            offset_date=0,
            offset_message_id=0,
            offset_message_thread_id=0,
            limit=100,
        )
    except Exception:
        return None
    for topic in result.topics or []:
        info = getattr(topic, "info", None)
        if info is not None and info.name == name:
            return info.message_thread_id
    return None


async def create_machine_topic(
    client: Client,
    *,
    chat_id: int,
    machine_name: str,
) -> CreatedTopic:
    """Create the per-machine forum topic, or adopt an existing one with the
    same name."""
    name = normalize_project_name(machine_name)

    existing_id = await find_topic_by_name(client, chat_id=chat_id, name=name)
    if existing_id is not None:
        logger.info("reusing existing topic %s -> message_thread_id=%s", name, existing_id)
        return CreatedTopic(
            name=name,
            topic_id=existing_id,
            is_new=False,
        )

    info = await client.api.create_forum_topic(
        chat_id=chat_id,
        name=name,
        icon=_icon_for(name),
    )
    logger.info("created topic %s -> message_thread_id=%s", name, info.message_thread_id)
    return CreatedTopic(
        name=name,
        topic_id=info.message_thread_id,
        is_new=True,
    )


async def seed_welcome(
    client: Client,
    *,
    chat_id: int,
    topic: CreatedTopic,
) -> CreatedTopic:
    """Post a welcome message into the topic and record its message id."""
    text = _welcome_text(topic.name)
    msg = await client.api.send_message(
        chat_id=chat_id,
        message_thread_id=topic.topic_id,
        input_message_content=InputMessageText(
            text=FormattedText(text=text, entities=[]),
            clear_draft=True,
        ),
    )
    topic.welcome_message_id = msg.id
    return topic
