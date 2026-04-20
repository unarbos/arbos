"""Thin wrapper around :class:`aiotdlib.Client` configured for this install.

The wrapper owns the ``ClientSettings`` construction and exposes a context-
manager helper that starts the client (which transparently runs the auth
flow) and stops it on exit. It deliberately does not hide the underlying
client; callers can reach into ``.client`` for ad-hoc API calls.
"""

from __future__ import annotations

import asyncio
import contextlib
import ctypes
import json
from typing import AsyncIterator, Optional

from aiotdlib import Client
from aiotdlib.client import ClientSettings

from .config import InstallInputs
from .workspace import InstallPaths


_TDLIB_SILENCED = False


def silence_tdlib() -> None:
    """Drop libtdjson's log verbosity to FATAL before any client is created.

    aiotdlib only calls ``setLogVerbosityLevel`` *after* a client is started,
    which means libtdjson's default stderr writer still emits its level-3
    startup line ("Created managed client N", "Client.cpp:482", ...). We
    pre-empt that by talking directly to the bundled tdjson via ctypes and
    calling ``td_execute({"@type":"setLogVerbosityLevel","new_verbosity_level":0})``
    once, before any :class:`aiotdlib.Client` exists. Idempotent.
    """
    global _TDLIB_SILENCED
    if _TDLIB_SILENCED:
        return
    try:
        from aiotdlib.tdjson import _get_bundled_tdjson_lib_path  # type: ignore
    except Exception:
        return
    try:
        lib = ctypes.CDLL(_get_bundled_tdjson_lib_path())
        lib.td_execute.restype = ctypes.c_char_p
        lib.td_execute.argtypes = [ctypes.c_char_p]
        lib.td_execute(
            json.dumps(
                {"@type": "setLogVerbosityLevel", "new_verbosity_level": 0}
            ).encode("utf-8")
        )
        _TDLIB_SILENCED = True
    except Exception:
        # If silencing fails we'd rather get the noise than crash the install.
        return


def build_settings(
    inputs: InstallInputs,
    paths: InstallPaths,
) -> ClientSettings:
    return ClientSettings(
        api_id=inputs.telegram_api_id,
        api_hash=inputs.telegram_api_hash,
        phone_number=inputs.phone,
        password=inputs.twofa_password,
        files_directory=paths.tdlib,
        device_model="arbos",
        application_version="0.1.0",
        system_language_code="en",
        use_file_database=True,
        use_chat_info_database=True,
        use_message_database=True,
        use_secret_chats=False,
    )


class ArbosSession:
    """Manages the lifecycle of a single aiotdlib ``Client`` instance."""

    def __init__(self, inputs: InstallInputs, paths: InstallPaths) -> None:
        self._settings = build_settings(inputs, paths)
        self._client: Optional[Client] = None

    @property
    def client(self) -> Client:
        if self._client is None:
            raise RuntimeError("ArbosSession not started")
        return self._client

    async def __aenter__(self) -> Client:
        silence_tdlib()
        self._client = Client(settings=self._settings)
        await self._client.start()
        return self._client

    async def __aexit__(self, exc_type, exc, tb) -> None:
        if self._client is not None:
            # aiotdlib's stop() awaits the update-loop task that it just
            # cancelled, which re-raises CancelledError at our boundary.
            # That is expected shutdown noise; we only re-raise cancellation
            # if our own task is also being cancelled (Ctrl-C, parent timeout).
            try:
                await self._client.stop()
            except asyncio.CancelledError:
                task = asyncio.current_task()
                if task is not None and task.cancelling():
                    raise
            except Exception:
                pass
            self._client = None


@contextlib.asynccontextmanager
async def open_session(
    inputs: InstallInputs,
    paths: InstallPaths,
) -> AsyncIterator[Client]:
    """Async-context helper: ``async with open_session(...) as client: ...``."""
    session = ArbosSession(inputs, paths)
    async with session as client:
        yield client
