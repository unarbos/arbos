"""Pure call lifetime and independently applied mode/mute state.

SDK effects remain in VoiceCall. Attempt tokens reject late completions; failed effects
retain the last confirmed value without claiming that the hardware is still known.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Generic, Literal, TypeVar

CLEANUP_STEPS = ("listeners", "vision", "updates", "session", "tasks")


@dataclass(frozen=True)
class Connecting:
    _tag: Literal["Connecting"] = field(default="Connecting", init=False)


@dataclass(frozen=True)
class WaitingForHuman:
    _tag: Literal["WaitingForHuman"] = field(default="WaitingForHuman", init=False)


@dataclass(frozen=True)
class StartingSession:
    _tag: Literal["StartingSession"] = field(default="StartingSession", init=False)


@dataclass(frozen=True)
class Running:
    _tag: Literal["Running"] = field(default="Running", init=False)


@dataclass(frozen=True)
class Finishing:
    reason: str
    attempt: int
    pending_steps: tuple[str, ...]
    _tag: Literal["Finishing"] = field(default="Finishing", init=False)


@dataclass(frozen=True)
class CleanupFailed:
    reason: str
    attempt: int
    pending_steps: tuple[str, ...]
    _tag: Literal["CleanupFailed"] = field(default="CleanupFailed", init=False)


@dataclass(frozen=True)
class Closed:
    reason: str
    attempt: int
    _tag: Literal["Closed"] = field(default="Closed", init=False)


Lifecycle = Connecting | WaitingForHuman | StartingSession | Running | Finishing | CleanupFailed | Closed


@dataclass(frozen=True)
class StartupProgress:
    stage: Literal["waiting", "starting", "running"]


@dataclass(frozen=True)
class StopRequested:
    reason: str


@dataclass(frozen=True)
class Finalized:
    attempt: int
    failed_steps: tuple[str, ...]


@dataclass(frozen=True)
class PreSessionStopped:
    reason: str


def lifecycle_transition(state: Lifecycle, event: StartupProgress | StopRequested | Finalized | PreSessionStopped) -> Lifecycle:
    if isinstance(state, Closed):
        return state
    if isinstance(event, StopRequested):
        if isinstance(state, Finishing):
            return state
        if isinstance(state, CleanupFailed):
            return Finishing(state.reason, state.attempt + 1, state.pending_steps)
        return Finishing(event.reason, 1, CLEANUP_STEPS)
    if isinstance(event, Finalized):
        if not isinstance(state, Finishing) or state.attempt != event.attempt:
            return state
        if not set(event.failed_steps).issubset(state.pending_steps):
            raise ValueError("Finalization may only name cleanup steps owned by this attempt")
        failed = tuple(step for step in state.pending_steps if step in event.failed_steps)
        return CleanupFailed(state.reason, state.attempt, failed) if failed else Closed(state.reason, state.attempt)
    if isinstance(event, PreSessionStopped):
        return Closed(event.reason, 0) if isinstance(state, (Connecting, WaitingForHuman, StartingSession)) else state
    if isinstance(event, StartupProgress):
        if event.stage == "waiting" and isinstance(state, Connecting):
            return WaitingForHuman()
        if event.stage == "starting" and isinstance(state, (Connecting, WaitingForHuman)):
            return StartingSession()
        if event.stage == "running" and isinstance(state, StartingSession):
            return Running()
    return state


@dataclass(frozen=True)
class Group:
    _tag: Literal["Group"] = field(default="Group", init=False)


@dataclass(frozen=True)
class Duplex:
    identity: str
    _tag: Literal["Duplex"] = field(default="Duplex", init=False)

    def __post_init__(self) -> None:
        if not isinstance(self.identity, str) or not self.identity:
            raise ValueError("Duplex requires a participant identity")


ModeValue = Group | Duplex
T = TypeVar("T")


@dataclass(frozen=True)
class Unapplied(Generic[T]):
    value: T | None = None
    attempt: int = 0
    _tag: Literal["Unapplied"] = field(default="Unapplied", init=False)


@dataclass(frozen=True)
class Applied(Generic[T]):
    value: T
    attempt: int
    _tag: Literal["Applied"] = field(default="Applied", init=False)

    def __post_init__(self) -> None:
        if self.value is None:
            raise ValueError("A confirmed value cannot be missing")


@dataclass(frozen=True)
class Applying(Generic[T]):
    previous: T | None
    target: T
    attempt: int
    _tag: Literal["Applying"] = field(default="Applying", init=False)


@dataclass(frozen=True)
class ApplyFailed(Generic[T]):
    previous: T | None
    target: T
    attempt: int
    _tag: Literal["ApplyFailed"] = field(default="ApplyFailed", init=False)


ApplyState = Unapplied[T] | Applied[T] | Applying[T] | ApplyFailed[T]


@dataclass(frozen=True)
class ApplyRequested(Generic[T]):
    value: T


@dataclass(frozen=True)
class ApplySucceeded:
    attempt: int


@dataclass(frozen=True)
class ApplyErrored:
    attempt: int


def confirmed_value(state: ApplyState[T]) -> T | None:
    if isinstance(state, Applied):
        return state.value
    return state.previous if isinstance(state, (Applying, ApplyFailed)) else None


def requested_value(state: ApplyState[T]) -> T | None:
    return state.target if isinstance(state, (Applying, ApplyFailed)) else state.value


def apply_transition(state: ApplyState[T], event: ApplyRequested[T] | ApplySucceeded | ApplyErrored) -> ApplyState[T]:
    if isinstance(event, ApplyRequested):
        if event.value is None:
            raise ValueError("An effect requires a target")
        if isinstance(state, Applied) and state.value == event.value:
            return state
        if isinstance(state, Applying) and state.target == event.value:
            return state
        return Applying(confirmed_value(state), event.value, state.attempt + 1)
    if not isinstance(state, Applying) or state.attempt != event.attempt:
        return state
    if isinstance(event, ApplySucceeded):
        return Applied(state.target, state.attempt)
    return ApplyFailed(state.previous, state.target, state.attempt)
