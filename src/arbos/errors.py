"""Domain errors raised by the installer.

Two broad classes mirror the state machine's terminal failure states:
- ``RecoverableInstallError`` -> the caller may retry the same phase later
- ``FatalInstallError`` -> the installer should stop and surface to the user
"""

from __future__ import annotations


class InstallError(Exception):
    """Base class for installer-specific errors."""


class RecoverableInstallError(InstallError):
    """A failure that should leave the install state at the previous boundary
    so a re-run of ``arbos install`` can resume from there."""


class FatalInstallError(InstallError):
    """A failure the installer cannot meaningfully retry on its own."""


class BotFatherParseError(FatalInstallError):
    """BotFather's reply did not match any expected shape."""


class UsernameExhaustedError(FatalInstallError):
    """Ran out of username candidates while talking to BotFather."""


class AuthTimeoutError(RecoverableInstallError):
    """Arbos authorization did not complete in the allotted time."""
