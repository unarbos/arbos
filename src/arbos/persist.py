"""Final persistence layer.

Writes ``config.json`` (per the spec shape, single-machine version), an
append-only ``install.log``, and a sidecar ``doppler_writeback.json`` that
``run.sh`` consumes after the installer exits to push newly-discovered values
back into Doppler.
"""

from __future__ import annotations

import json
import logging
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Mapping

from .config import (
    BotSection,
    InstallInputs,
    MachineSection,
    StoredConfig,
    SupergroupSection,
    TelegramSection,
)
from .topics import CreatedTopic
from .workspace import InstallPaths

logger = logging.getLogger(__name__)


def write_secrets(
    paths: InstallPaths,
    *,
    api_hash: str,
    bot_token: str,
) -> None:
    paths.api_hash_path.write_text(api_hash)
    paths.bot_token_path.write_text(bot_token)
    for path in (paths.api_hash_path, paths.bot_token_path):
        try:
            os.chmod(path, 0o600)
        except OSError:
            pass


def build_stored_config(
    *,
    paths: InstallPaths,
    inputs: InstallInputs,
    bot_username: str,
    bot_token: str,
    bot_user_id: int,
    supergroup_chat_id: int,
    supergroup_id: int,
    supergroup_title: str,
    is_forum: bool,
    machine_topic: CreatedTopic,
) -> StoredConfig:
    return StoredConfig(
        install_root=str(paths.arbos),
        machine=MachineSection(
            name=machine_topic.name,
            topic_id=machine_topic.topic_id,
            welcome_message_id=machine_topic.welcome_message_id,
        ),
        telegram=TelegramSection(
            api_id=str(inputs.telegram_api_id),
            api_hash=inputs.telegram_api_hash,
            arbos_session_path=str(paths.tdlib),
        ),
        bot=BotSection(
            username=bot_username,
            token=bot_token,
            user_id=bot_user_id,
        ),
        supergroup=SupergroupSection(
            title=supergroup_title,
            chat_id=supergroup_chat_id,
            supergroup_id=supergroup_id,
            is_forum=is_forum,
        ),
    )


def write_final_config(paths: InstallPaths, config: StoredConfig) -> Path:
    config.write(paths.config_path)
    try:
        os.chmod(paths.config_path, 0o600)
    except OSError:
        pass
    return paths.config_path


def write_doppler_writeback(paths: InstallPaths, entries: Mapping[str, str | int | None]) -> Path:
    """Drop a sidecar JSON file for ``run.sh`` to forward into Doppler.

    Empty / ``None`` values are filtered out. Existing keys with the same
    value are still emitted; ``run.sh`` no-ops on equal values via doppler.
    """
    payload = {k: ("" if v is None else str(v)) for k, v in entries.items() if v not in (None, "")}
    paths.writeback_path.write_text(json.dumps(payload, indent=2, sort_keys=True))
    return paths.writeback_path


def append_log(paths: InstallPaths, message: str) -> None:
    paths.log_path.parent.mkdir(parents=True, exist_ok=True)
    ts = datetime.now(timezone.utc).isoformat()
    with paths.log_path.open("a", encoding="utf-8") as f:
        f.write(f"{ts}  {message}\n")
