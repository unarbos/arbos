"""Arbos user-account login.

The heavy lifting (phone -> code -> 2FA prompts) is already implemented inside
aiotdlib's ``Client.start()`` flow, which uses ``stdin`` prompts when the
phone number is configured but the auth state is not yet ready. This module
therefore just calls ``client.start()`` and confirms the resulting account
identity, persisting ``ARBOS_AUTH_COMPLETE`` on success.
"""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass

from aiotdlib import Client

from .errors import AuthTimeoutError

logger = logging.getLogger(__name__)


@dataclass
class AuthorizedUser:
    user_id: int
    first_name: str
    last_name: str
    phone_number: str


async def confirm_authorized(client: Client, *, timeout: float = 60.0) -> AuthorizedUser:
    """Wait for ``client`` to be authorized and return the logged-in user."""
    try:
        user_id = await asyncio.wait_for(client.get_my_id(), timeout=timeout)
    except asyncio.TimeoutError as exc:
        raise AuthTimeoutError(
            f"Arbos authorization did not complete within {timeout:.0f}s"
        ) from exc

    user = await client.get_user(user_id)
    return AuthorizedUser(
        user_id=user.id,
        first_name=user.first_name,
        last_name=user.last_name or "",
        phone_number=user.phone_number or "",
    )
