"""The money of a call is taken while the call is up.

Jacob, 2026-11-05: "all limits should be cost based." A limit that is only checked at hang-up
cannot end anything, so the thing to hold is that `usage_loop` keeps reporting on the call's own
path and leaves the moment the relay answers with a cost stop — and that an account with no limit
is never the one that gets hung up on.

    source .venv/bin/activate && python -m pytest tests/test_usage_loop.py -q
"""

from __future__ import annotations

import asyncio

import pytest

from arbos_voice.worker import GOODBYE, VoiceCall

pytest_plugins = ("pytest_asyncio",)


class FakeRelay:
    """The relay as the loop sees it: it records each report and answers a scripted stop."""

    def __init__(self, stops: list[str | None]):
        self.stops = list(stops)
        self.reports: list[dict] = []

    async def usage(self, call_id: str, project_id: str, usage: dict) -> dict:
        self.reports.append(usage)
        stop = self.stops.pop(0) if self.stops else None
        return {"caps": {"stop": stop}, "usd_charged": usage.get("usd", 0.0)}


class FakeMeter:
    def __init__(self, seconds: float = 30.0, usd: float = 0.5):
        self._seconds = seconds
        self._usd = usd
        self.stop_reason: str | None = None

    def seconds(self) -> float:
        return self._seconds

    def usage(self, *, ended: bool = False) -> dict:
        return {"seconds": self._seconds, "usd": self._usd, "ended": ended}


class FakeState:
    def __init__(self):
        self.leaving = False
        self.stop_reason: str | None = None


class FakeCfg:
    """Real defaults in miniature: a short interval so the test is quick, a leak guard far out."""

    usage_every_s = 0.01
    call_leak_guard_s = 21600.0


class LoopHarness:
    """Just enough of `VoiceCall` for its real `usage_loop` to run against."""

    def __init__(self, relay: FakeRelay, meter: FakeMeter):
        self.relay = relay
        self.meter = meter
        self.state = FakeState()
        self.cfg = FakeCfg()
        self.call_id = "call_1"
        self.project_id = "p_1"
        self.finished: list[str] = []

    def sync_vision_meter(self) -> None:
        pass

    async def finish(self, reason: str) -> None:
        self.finished.append(reason)
        self.state.leaving = True
        self.state.stop_reason = reason


@pytest.mark.asyncio
async def test_the_loop_reports_while_the_call_is_up_and_leaves_on_a_cost_stop():
    # Two passes with room, then the relay answers the account's limit.
    relay = FakeRelay([None, None, "daily_cap"])
    h = LoopHarness(relay, FakeMeter())
    await asyncio.wait_for(VoiceCall.usage_loop(h), timeout=5)

    assert len(relay.reports) == 3, f"reported every interval, not once at the end: {relay.reports}"
    assert all(r["ended"] is False for r in relay.reports), "these are mid-call reports"
    assert h.finished == ["daily_cap"], "the call ends on the stop the relay answered"
    assert GOODBYE["daily_cap"], "and the agent has something to say before it goes"


@pytest.mark.asyncio
async def test_an_unlimited_account_is_never_hung_up_on_by_the_metering():
    # The relay never answers a stop: an account with no limit keeps talking however long and
    # however expensive the call gets. This is the regression a zero-for-unlimited would cause.
    relay = FakeRelay([])
    h = LoopHarness(relay, FakeMeter(seconds=7200.0, usd=120.0))

    task = asyncio.create_task(VoiceCall.usage_loop(h))
    await asyncio.sleep(0.2)
    assert h.finished == [], "no limit, no reason to leave"
    assert len(relay.reports) > 5, "and it kept charging all the while"

    h.state.leaving = True
    await asyncio.wait_for(task, timeout=5)


@pytest.mark.asyncio
async def test_the_leak_guard_is_far_above_any_real_call_and_is_not_a_budget():
    relay = FakeRelay([])
    # Six hours of wedged call, with the relay answering no stop at all.
    h = LoopHarness(relay, FakeMeter(seconds=FakeCfg.call_leak_guard_s + 1.0, usd=0.0))
    await asyncio.wait_for(VoiceCall.usage_loop(h), timeout=5)
    assert h.finished == ["leak_guard"], "only a wedged call is ended by time"

    # An hour, which used to be the cap, is not: cost is what ends a call now.
    relay = FakeRelay([])
    h = LoopHarness(relay, FakeMeter(seconds=3601.0, usd=50.0))
    task = asyncio.create_task(VoiceCall.usage_loop(h))
    await asyncio.sleep(0.05)
    assert h.finished == [], "an hour-long call is no longer stopped for being an hour long"
    h.state.leaving = True
    await asyncio.wait_for(task, timeout=5)
