"""Pydantic models for installer inputs and the on-disk ``config.json``."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Optional

from pydantic import BaseModel, Field, field_validator, model_validator

from .workspace import normalize_project_name


class InstallInputs(BaseModel):
    """User-supplied + machine-derived inputs for one install run.

    The supergroup is shared across all of this user's machines, so
    ``supergroup_chat_id`` / ``supergroup_id`` may be pre-populated by an
    earlier machine and propagated through Doppler. Likewise ``bot_token`` is
    pre-populated from the per-machine Doppler key when present.
    """

    telegram_api_id: int
    telegram_api_hash: str
    phone: str
    twofa_password: Optional[str] = None
    supergroup_title: str

    machine_name: str
    bot_username: str = ""
    bot_display_name: str = ""

    bot_token: Optional[str] = None
    supergroup_chat_id: Optional[int] = None
    supergroup_id: Optional[int] = None

    @field_validator("machine_name")
    @classmethod
    def _normalize_machine(cls, v: str) -> str:
        return normalize_project_name(v)

    @field_validator("phone")
    @classmethod
    def _check_phone(cls, v: str) -> str:
        v = v.strip()
        if not v.startswith("+"):
            raise ValueError("phone must be in international format starting with +")
        return v

    @model_validator(mode="after")
    def _derive_bot_fields(self) -> "InstallInputs":
        if not self.bot_username:
            self.bot_username = f"arbos_{self.machine_name}_bot"
        else:
            self.bot_username = self.bot_username.strip().lstrip("@")
        if not self.bot_username.endswith("bot"):
            raise ValueError(f"bot_username {self.bot_username!r} must end with 'bot'")

        if not self.bot_display_name:
            self.bot_display_name = f"arbos {self.machine_name}"
        return self


class TelegramSection(BaseModel):
    api_id: str
    api_hash: str
    arbos_session_path: str


class BotSection(BaseModel):
    username: str
    token: str
    user_id: Optional[int] = None


class SupergroupSection(BaseModel):
    title: str
    chat_id: int
    supergroup_id: int
    is_forum: bool = True


class MachineSection(BaseModel):
    name: str
    topic_id: int
    welcome_message_id: Optional[int] = None


class StoredConfig(BaseModel):
    """Final on-disk shape, single-machine version."""

    install_root: str
    machine: MachineSection
    telegram: TelegramSection
    bot: BotSection
    supergroup: SupergroupSection = Field(...)

    def write(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(self.model_dump_json(indent=2))

    @classmethod
    def read(cls, path: Path) -> "StoredConfig":
        return cls.model_validate(json.loads(path.read_text()))
