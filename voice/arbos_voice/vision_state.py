"""Pure consent/lifetime state. Encoders borrow a generation; they never own consent."""

from dataclasses import dataclass
from typing import Literal


@dataclass(frozen=True)
class Enabled:
    generation: int = 0


@dataclass(frozen=True)
class Disabled:
    generation: int = 0


@dataclass(frozen=True)
class Closed:
    generation: int = 0


VisionState = Enabled | Disabled | Closed
Event = Literal["enable", "disable", "close"]


def transition(state: VisionState, event: Event) -> VisionState:
    if event not in ("enable", "disable", "close"):
        raise ValueError("Unknown vision event")
    if isinstance(state, Closed):
        return state
    if event == "close":
        return Closed(state.generation + 1)
    if event == "enable" and isinstance(state, Disabled):
        return Enabled(state.generation + 1)
    if event == "disable" and isinstance(state, Enabled):
        return Disabled(state.generation + 1)
    return state


def capture_generation(state: VisionState) -> int | None:
    return state.generation if isinstance(state, Enabled) else None


def accepts_frame(state: VisionState, generation: int | None) -> bool:
    return isinstance(state, Enabled) and generation == state.generation
