"""Pure commit-admission and delivery lifetimes used by the feedback controller."""
from __future__ import annotations

from dataclasses import dataclass
import datetime as dt
import re
from typing import Union


def valid_sha(value) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value) is not None


def approved(value) -> bool:
    if not isinstance(value, str) or len(value) > 64:
        return False
    try:
        dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
        return True
    except ValueError:
        return False


@dataclass(frozen=True)
class Candidate:
    head: str


@dataclass(frozen=True)
class Held:
    head: str


@dataclass(frozen=True)
class AwaitingApproval:
    head: str


@dataclass(frozen=True)
class GatePending:
    head: str


@dataclass(frozen=True)
class MissingHead:
    head: str


@dataclass(frozen=True)
class StaleHead:
    head: str
    live: str


@dataclass(frozen=True)
class ReadyToMerge:
    head: str


@dataclass(frozen=True)
class Landed:
    head: str
    merge: str


@dataclass(frozen=True)
class InvalidControl:
    reason: str


@dataclass(frozen=True)
class ObserveGate:
    covered_head: str | None
    live_head: str | None
    green: bool
    eligible: bool
    held: bool
    approval_required: bool


@dataclass(frozen=True)
class Merged:
    sha: str


CommitState = Union[Candidate, Held, AwaitingApproval, GatePending, MissingHead, StaleHead, ReadyToMerge, Landed, InvalidControl]


def commit_transition(state: CommitState, event: ObserveGate | Merged) -> CommitState:
    if isinstance(state, (Landed, InvalidControl)):
        return state
    if not valid_sha(state.head):
        return InvalidControl("invalid branch head")
    if isinstance(event, ObserveGate):
        if any(type(value) is not bool for value in (event.green, event.eligible, event.held, event.approval_required)):
            return InvalidControl("invalid gate flags")
        if event.held:
            return Held(state.head)
        if not event.eligible or not event.green or event.covered_head != state.head:
            return GatePending(state.head)
        if not valid_sha(event.live_head):
            return MissingHead(state.head)
        if event.live_head != state.head:
            return StaleHead(state.head, event.live_head)
        if event.approval_required:
            return AwaitingApproval(state.head)
        return ReadyToMerge(state.head)
    if isinstance(state, ReadyToMerge) and isinstance(event, Merged) and valid_sha(event.sha):
        return Landed(state.head, event.sha)
    return InvalidControl("invalid commit transition")


@dataclass(frozen=True)
class DeliveryPending:
    commit: str


@dataclass(frozen=True)
class AwaitingArtifacts:
    commit: str
    pending_deploy: tuple[str, ...]
    device_evidence: str


@dataclass(frozen=True)
class AwaitingDeploy:
    commit: str
    pending_deploy: tuple[str, ...]
    device_evidence: str

    def __post_init__(self):
        if not self.pending_deploy:
            raise ValueError("AwaitingDeploy must retain at least one required deployment")


@dataclass(frozen=True)
class Shipped:
    commit: str
    device_evidence: str


@dataclass(frozen=True)
class ObserveDelivery:
    pending_deploy: tuple[str, ...]
    waiting_ios: bool
    device_evidence: str


@dataclass(frozen=True)
class Deployed:
    commit: str


DeliveryState = Union[DeliveryPending, AwaitingArtifacts, AwaitingDeploy, Shipped, InvalidControl]


def delivery_transition(state: DeliveryState, event: ObserveDelivery | Deployed) -> DeliveryState:
    if isinstance(state, (Shipped, InvalidControl)):
        return state
    if not valid_sha(state.commit):
        return InvalidControl("invalid delivery commit")
    if isinstance(event, ObserveDelivery):
        if (type(event.waiting_ios) is not bool or not isinstance(event.pending_deploy, tuple)
                or any(kind not in ("voice", "agentd", "relay") for kind in event.pending_deploy)
                or len(set(event.pending_deploy)) != len(event.pending_deploy)
                or not isinstance(event.device_evidence, str) or not event.device_evidence):
            return InvalidControl("invalid delivery evidence")
        if event.waiting_ios:
            return AwaitingArtifacts(state.commit, event.pending_deploy, event.device_evidence)
        if event.pending_deploy:
            return AwaitingDeploy(state.commit, event.pending_deploy, event.device_evidence)
        return Shipped(state.commit, event.device_evidence)
    if isinstance(event, Deployed) and event.commit == state.commit:
        if isinstance(state, AwaitingArtifacts):
            return AwaitingArtifacts(state.commit, (), state.device_evidence)
        if isinstance(state, AwaitingDeploy):
            return Shipped(state.commit, state.device_evidence)
    return InvalidControl("invalid delivery transition")
