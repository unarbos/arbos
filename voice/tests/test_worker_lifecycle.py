"""Real VoiceCall methods with deterministic local SDK effects, never provider/device I/O."""
from __future__ import annotations

import asyncio
import time
from dataclasses import replace
from copy import deepcopy
from types import SimpleNamespace

import pytest

from arbos_voice.call_state import Running
from arbos_voice.config import Config
from arbos_voice.worker import VoiceCall

pytest_plugins = ("pytest_asyncio",)


class Resource:
    def __init__(self, *, fail_once=False, block=False):
        self.calls = 0
        self.fail_once = fail_once
        self.started = asyncio.Event()
        self.release = asyncio.Event()
        if not block:
            self.release.set()

    def anyone_speaking(self):
        return False

    async def stop(self):
        self.calls += 1
        self.started.set()
        await self.release.wait()
        if self.fail_once:
            self.fail_once = False
            raise RuntimeError("synthetic local cleanup failure")


class Audio:
    def __init__(self):
        self.enabled = True
        self.calls = []
        self.fail_once = False

    def set_audio_enabled(self, enabled):
        self.calls.append(enabled)
        self.enabled = enabled  # Failure may follow a partially applied SDK effect.
        if self.fail_once:
            self.fail_once = False
            raise RuntimeError("synthetic audio effect failure")


class Agent:
    def __init__(self):
        self.realtime_llm_session = SimpleNamespace(update_options=lambda **kwargs: None)
        self.fail_once = False
        self.instructions = []
        self.started = asyncio.Event()
        self.release = asyncio.Event()
        self.release.set()

    async def update_instructions(self, value):
        self.started.set()
        self.instructions.append(value)
        await self.release.wait()
        if self.fail_once:
            self.fail_once = False
            raise RuntimeError("synthetic instructions effect failure")


def make_call(*, started=True):
    shutdowns = []
    room = SimpleNamespace(name="synthetic", local_participant=SimpleNamespace(identity="a_synthetic"), remote_participants={"u_first": SimpleNamespace(identity="u_first", metadata="", name="First")})
    ctx = SimpleNamespace(room=room, proc=SimpleNamespace(userdata={}), shutdown=lambda **kwargs: shutdowns.append(kwargs["reason"]))
    cfg = Config(
        relay_url="http://127.0.0.1:7700", relay_token="synthetic",
        openai_api_key="synthetic", livekit_url="", livekit_api_key="", livekit_api_secret="",
    )
    call = VoiceCall(ctx, cfg, {"call_id": "synthetic", "project_id": "synthetic", "chat_id": "synthetic"}, None)
    if started:
        call.state.lifecycle = Running()
    calls = {"usage": [], "events": [], "closed": 0, "session_closed": 0, "participants": []}

    async def usage(*args):
        calls["usage"].append(deepcopy(args[-1]))
        return {}

    async def close_events():
        calls["closed"] += 1

    async def close_session():
        calls["session_closed"] += 1

    call.relay = SimpleNamespace(usage=usage)
    call.events = SimpleNamespace(push=lambda value: calls["events"].append(value), close=close_events, start=lambda: None)
    call.session = SimpleNamespace(
        input=Audio(), output=Audio(), interrupt=lambda: None,
        update_options=lambda **kwargs: None, aclose=close_session,
        room_io=SimpleNamespace(set_participant=lambda value: calls["participants"].append(value)),
    )
    call.agent = Agent()
    call.rt_model = object()

    async def start_group():
        if call.listeners is None:
            call.listeners = Resource()
        await asyncio.sleep(0)

    call.start_group_listening = start_group
    return call, calls, shutdowns


@pytest.mark.asyncio
async def test_cleanup_failure_finalizes_and_retry_does_not_repeat_successful_edges():
    call, calls, shutdowns = make_call()
    listener = call.listeners = Resource(fail_once=True)
    await call.finish("alone")
    assert call._done.is_set()
    assert shutdowns == ["alone"]
    assert len(calls["usage"]) == 1 and calls["usage"][0]["ended"] is True
    assert calls["session_closed"] == 1
    assert len([event for event in calls["events"] if event["kind"] == "left"]) == 1
    await call.finish("different_reason")
    assert listener.calls == 2
    assert shutdowns == ["alone"]
    assert len(calls["usage"]) == 1
    assert calls["session_closed"] == 1
    assert len([event for event in calls["events"] if event["kind"] == "left"]) == 1


@pytest.mark.asyncio
async def test_cancelling_a_finish_caller_cannot_abort_owned_cleanup():
    call, calls, shutdowns = make_call()
    listener = call.listeners = Resource(block=True)
    caller = asyncio.create_task(call.finish("alone"))
    await listener.started.wait()
    caller.cancel()
    with pytest.raises(asyncio.CancelledError):
        await caller
    listener.release.set()
    await asyncio.wait_for(call._done.wait(), timeout=0.5)
    await call.finish("ignored")
    assert shutdowns == ["alone"]
    assert len(calls["usage"]) == 1


@pytest.mark.asyncio
async def test_concurrent_finish_callers_wait_for_one_first_reason_cleanup():
    call, calls, shutdowns = make_call()
    listener = call.listeners = Resource(block=True)
    first = asyncio.create_task(call.finish("alone"))
    await listener.started.wait()
    second = asyncio.create_task(call.finish("different_reason"))
    await asyncio.sleep(0)
    assert not second.done(), "second caller must observe completion, not merely leaving=true"
    listener.release.set()
    await asyncio.gather(first, second)
    assert listener.calls == 1 and len(calls["usage"]) == 1
    assert shutdowns == ["alone"]


@pytest.mark.parametrize("target", [True, False])
@pytest.mark.asyncio
async def test_failed_mute_retries_same_target_instead_of_reporting_already_applied(target):
    call, _calls, _shutdowns = make_call()
    await call.set_muted(not target)
    call.session.output.calls.clear()
    call.session.output.fail_once = True
    assert "Could not" in await call.set_muted(target)
    assert call.state.muted is (not target), "the last confirmed output state must survive SDK failure"
    assert "Already" not in await call.set_muted(target)
    assert call.session.output.calls == [not target, not target]
    assert call.state.muted is target


@pytest.mark.parametrize("target", [True, False])
@pytest.mark.asyncio
async def test_failed_mute_opposite_target_reapplies_even_if_last_confirmed_value_matches(target):
    call, _calls, _shutdowns = make_call()
    await call.set_muted(not target)
    call.session.output.calls.clear()
    call.session.output.fail_once = True
    assert "Could not" in await call.set_muted(target)
    assert "Already" not in await call.set_muted(not target)
    assert call.session.output.calls == [not target, target]
    assert call.session.output.enabled is target


@pytest.mark.asyncio
async def test_partial_mode_failure_keeps_confirmed_route_and_allows_same_target_retry():
    call, _calls, _shutdowns = make_call()
    await call.switch_mode("duplex")
    call.ctx.room.remote_participants["u_second"] = SimpleNamespace(identity="u_second", metadata="", name="Second")
    call.agent.fail_once = True
    with pytest.raises(RuntimeError, match="instructions"):
        await call.switch_mode("group")
    assert call.state.mode == "duplex"
    assert call.state.linked_identity == "u_first"
    await call.switch_mode("group")
    assert call.state.mode == "group" and call.state.linked_identity is None


@pytest.mark.asyncio
async def test_partial_mode_failure_can_reconcile_to_the_previous_confirmed_route():
    call, _calls, _shutdowns = make_call()
    await call.switch_mode("duplex")
    call.ctx.room.remote_participants["u_second"] = SimpleNamespace(identity="u_second", metadata="", name="Second")
    call.agent.fail_once = True
    with pytest.raises(RuntimeError):
        await call.switch_mode("group")
    call.ctx.room.remote_participants.pop("u_second")
    await call.switch_mode("duplex")
    assert call.state.mode == "duplex" and call.state.linked_identity == "u_first"
    assert call.session.input.enabled is True


@pytest.mark.asyncio
async def test_mode_completion_after_close_cannot_publish_a_new_route_or_audio():
    call, calls, shutdowns = make_call()
    await call.switch_mode("duplex")
    call.ctx.room.remote_participants["u_second"] = SimpleNamespace(identity="u_second", metadata="", name="Second")
    call.agent.started.clear()
    call.agent.release.clear()
    mode_task = asyncio.create_task(call.switch_mode("group"))
    await call.agent.started.wait()
    await asyncio.wait_for(call.finish("alone"), timeout=0.5)
    call.agent.release.set()
    await asyncio.gather(mode_task, return_exceptions=True)
    assert call.state.mode == "duplex" and call.state.linked_identity == "u_first"
    assert call._done.is_set() and shutdowns == ["alone"]
    assert len(calls["usage"]) == 1


@pytest.mark.asyncio
async def test_event_flush_failure_is_best_effort_and_cannot_block_shutdown():
    call, calls, shutdowns = make_call()

    async def fail_close():
        calls["closed"] += 1
        raise RuntimeError("synthetic event flush failure")

    call.events.close = fail_close
    await call.finish("alone")
    await call.finish("different_reason")
    assert call._done.is_set() and shutdowns == ["alone"]
    assert calls["closed"] == 1 and len(calls["usage"]) == 1


@pytest.mark.asyncio
async def test_final_usage_failure_is_not_retried_when_local_cleanup_is_retried():
    call, calls, shutdowns = make_call()
    call.listeners = Resource(fail_once=True)

    async def fail_usage(*args):
        calls["usage"].append(deepcopy(args[-1]))
        raise RuntimeError("synthetic final report failure")

    call.relay.usage = fail_usage
    await call.finish("alone")
    frozen = deepcopy(calls["usage"][0])
    await call.finish("different_reason")
    assert calls["usage"] == [frozen]
    assert shutdowns == ["alone"] and call._done.is_set()


@pytest.mark.asyncio
async def test_pending_deferred_work_is_cancelled_and_no_new_work_starts_after_close():
    call, calls, _shutdowns = make_call()
    call.cfg = replace(call.cfg, tool_quick_wait_s=0.001)
    cancelled = asyncio.Event()
    started = []

    async def tool(*args, **kwargs):
        started.append(True)
        try:
            await asyncio.Event().wait()
        finally:
            cancelled.set()

    call.relay.tool = tool
    assert "working" in await call.ask_agent("synthetic request")
    assert call.judge_ctx().agent_has_open_work is True
    await call.finish("alone")
    assert cancelled.is_set() and call.judge_ctx().agent_has_open_work is False
    assert "ending" in await call.ask_agent("must not start")
    assert len(started) == 1 and len(calls["usage"]) == 1


@pytest.mark.asyncio
async def test_finish_spawned_as_owned_work_cannot_cancel_or_wait_on_itself():
    call, calls, shutdowns = make_call()
    caller = call.spawn(call.finish("alone"))
    await asyncio.wait_for(call._done.wait(), timeout=0.5)
    await asyncio.gather(caller, return_exceptions=True)
    assert shutdowns == ["alone"] and len(calls["usage"]) == 1


@pytest.mark.asyncio
async def test_closed_call_rejects_late_mute_without_touching_audio():
    call, _calls, _shutdowns = make_call()
    await call.finish("alone")
    call.session.output.calls.clear()
    assert "Could not" in await call.set_muted(True)
    assert call.session.output.calls == []


@pytest.mark.asyncio
async def test_failed_listener_start_keeps_the_allocated_resource_owned_until_cleanup():
    call, _calls, shutdowns = make_call()
    await call.switch_mode("duplex")
    listener = Resource(fail_once=True)

    async def fail_start():
        call.listeners = listener
        raise RuntimeError("synthetic listener startup failure")

    call.start_group_listening = fail_start
    with pytest.raises(RuntimeError, match="startup"):
        await call.switch_mode("group")
    assert call.listeners is listener and listener.calls == 1
    await call.finish("alone")
    assert listener.calls == 2 and shutdowns == ["alone"]


@pytest.mark.asyncio
async def test_relink_failure_commits_neither_mode_nor_participant_identity():
    call, calls, _shutdowns = make_call()
    await call.switch_mode("duplex")
    call.ctx.room.remote_participants = {"u_new": SimpleNamespace(identity="u_new", metadata="", name="New")}
    fail = True

    def participant(identity):
        nonlocal fail
        calls["participants"].append(identity)
        if fail:
            fail = False
            raise RuntimeError("synthetic participant effect failure")

    call.session.room_io.set_participant = participant
    with pytest.raises(RuntimeError, match="participant"):
        await call.switch_mode("duplex")
    assert call.state.mode == "duplex" and call.state.linked_identity == "u_first"
    await call.switch_mode("duplex")
    assert call.state.linked_identity == "u_new" and call.session.input.enabled is True


@pytest.mark.asyncio
async def test_roster_changes_serialize_and_recompute_the_latest_route_after_waiting():
    call, _calls, _shutdowns = make_call()
    await call.switch_mode("duplex")
    call.ctx.room.remote_participants["u_second"] = SimpleNamespace(identity="u_second", metadata="", name="Second")
    call.agent.started.clear()
    call.agent.release.clear()
    first = asyncio.create_task(call.on_humans_changed())
    await call.agent.started.wait()
    call.ctx.room.remote_participants.pop("u_second")
    second = asyncio.create_task(call.on_humans_changed())
    await asyncio.sleep(0.35)
    assert call.session.input.enabled is False, "a newer route must not interleave SDK effects"
    call.agent.release.set()
    await asyncio.gather(first, second)
    assert call.state.mode == "duplex" and call.state.linked_identity == "u_first"
    assert call.session.input.enabled is True


@pytest.mark.asyncio
async def test_pre_session_caps_stop_closes_without_starting_audio_or_final_usage():
    call, calls, shutdowns = make_call(started=False)
    call.session = None

    async def connect(**kwargs):
        pass

    async def info(*args):
        return {"caps": {"stop": "credits"}}

    call.ctx.connect = connect
    call.relay.call_info = info
    await call.run()
    await call.finish("shutdown")
    assert call.state.lifecycle._tag == "Closed" and call._done.is_set()
    assert calls["usage"] == [] and shutdowns == []


@pytest.mark.parametrize("close_during_start", [False, True, "greeting"])
@pytest.mark.asyncio
async def test_startup_does_not_claim_running_or_applied_route_before_sdk_start(monkeypatch, close_during_start):
    from arbos_voice import worker

    call, calls, shutdowns = make_call(started=False)
    session = call.session
    starting = asyncio.Event()
    release = asyncio.Event()
    greeting_started = asyncio.Event()
    greeting_release = asyncio.Event()

    async def start(*args, **kwargs):
        starting.set()
        await release.wait()

    async def connect(**kwargs):
        pass

    async def info(*args):
        return {}

    async def greet():
        if close_during_start == "greeting":
            greeting_started.set()
            await greeting_release.wait()

    session.start = start
    call.ctx.connect = connect
    call.relay.call_info = info
    call.hook_session = lambda session: None
    call.hook_room = call.hook_realtime = call.start_vision = call.start_updates = lambda: None
    call.greet = greet
    monkeypatch.setattr(worker, "ArbosVoiceAgent", lambda *args: Agent())
    monkeypatch.setattr(worker.openai.realtime, "RealtimeModel", lambda **kwargs: object())
    monkeypatch.setattr(worker, "AgentSession", lambda **kwargs: session)
    running = asyncio.create_task(call.run())
    await starting.wait()
    assert call.state.lifecycle._tag == "StartingSession"
    assert call.state.mode_application._tag == "Applying"
    if close_during_start is True:
        await asyncio.wait_for(call.finish("alone"), timeout=0.5)
        release.set()
        await asyncio.gather(running, return_exceptions=True)
        assert call.state.lifecycle._tag == "Closed"
        assert call.state.mode_application._tag == "ApplyFailed"
        assert call.state.linked_identity is None
        assert shutdowns == ["alone"] and len(calls["usage"]) == 1
        return
    release.set()
    for _ in range(20):
        if call.state.lifecycle._tag == "Running":
            break
        await asyncio.sleep(0)
    assert call.state.lifecycle._tag == "Running"
    assert call.state.mode == "duplex" and call.state.linked_identity == "u_first"
    if close_during_start == "greeting":
        await greeting_started.wait()
    await call.finish("alone")
    try:
        await asyncio.wait_for(running, timeout=0.5)
    finally:
        greeting_release.set()
    assert shutdowns == ["alone"] and len(calls["usage"]) == 1


@pytest.mark.asyncio
async def test_goodbye_can_play_while_stopping_and_is_not_repeated_on_cleanup_retry():
    call, calls, shutdowns = make_call()
    call.listeners = Resource(fail_once=True)
    spoken = []

    async def playout():
        assert call.state.leaving is True

    def reply(**kwargs):
        spoken.append(kwargs)
        return SimpleNamespace(wait_for_playout=playout)

    call.session.generate_reply = reply
    await call.finish("credits")
    await call.finish("ignored")
    assert len(spoken) == 1 and "There are no credits left" in spoken[0]["instructions"]
    assert spoken[0]["allow_interruptions"] is False
    assert shutdowns == ["credits"] and len(calls["usage"]) == 1


@pytest.mark.parametrize("stage", ["connect", "info", "human", "human_no_join"])
@pytest.mark.asyncio
async def test_late_startup_completion_after_close_cannot_create_sdk_audio(monkeypatch, stage):
    from arbos_voice import worker

    call, calls, shutdowns = make_call(started=False)
    call.session = None
    entered = asyncio.Event()
    release = asyncio.Event()
    constructed = []
    if stage in ("human", "human_no_join"):
        call.ctx.room.remote_participants.clear()
        call.events.start = entered.set

    async def connect(**kwargs):
        if stage == "connect":
            entered.set()
            await release.wait()

    async def info(*args):
        if stage == "info":
            entered.set()
            await release.wait()
        return {}

    def model(**kwargs):
        constructed.append(True)
        raise AssertionError("closed call tried to construct new SDK audio")

    call.ctx.connect = connect
    call.relay.call_info = info
    monkeypatch.setattr(worker, "ArbosVoiceAgent", lambda *args: Agent())
    monkeypatch.setattr(worker.openai.realtime, "RealtimeModel", model)
    running = asyncio.create_task(call.run())
    await entered.wait()
    await call.finish("alone")
    release.set()
    if stage == "human":
        call.ctx.room.remote_participants["u_late"] = SimpleNamespace(identity="u_late", metadata="", name="Late")
    await asyncio.wait_for(running, timeout=0.5)
    assert constructed == [] and call.state.lifecycle._tag == "Closed"
    assert shutdowns == ["alone"] and len(calls["usage"]) == 1


@pytest.mark.asyncio
async def test_uncertain_mute_does_not_allow_background_speech_from_the_last_confirmed_flag():
    call, _calls, _shutdowns = make_call()
    await call.set_muted(False)
    call.session.output.fail_once = True
    assert "Could not" in await call.set_muted(True)
    assert call.state.muted is False
    generated = []
    call.session.generate_reply = lambda **kwargs: generated.append(kwargs)
    await call.say("synthetic background update")
    assert generated == []


@pytest.mark.parametrize("stage", ["judge", "silence", "lock"])
@pytest.mark.parametrize("change", ["close", "mode", "mode_cycle"])
@pytest.mark.asyncio
async def test_late_group_work_cannot_generate_after_its_lifetime_or_mode_ownership_changes(stage, change):
    from arbos_voice.judge import Decision
    from arbos_voice.listener import Utterance

    call, _calls, _shutdowns = make_call()
    await call.set_muted(False)
    await call.switch_mode("group")
    entered = asyncio.Event()
    release = asyncio.Event()
    generated = []
    call.session.generate_reply = lambda **kwargs: generated.append(kwargs)
    if stage == "lock":
        await call._speak_lock.acquire()

    async def decide(*args):
        if stage in ("judge", "lock"):
            entered.set()
        if stage == "judge":
            await release.wait()
        return Decision(action="speak", forced=True)

    async def silence(*args):
        if stage == "silence":
            entered.set()
            await release.wait()

    call.judge = SimpleNamespace(decide=decide)
    call.wait_for_silence = silence
    utterance = Utterance("u_first", "synthetic words", 0.5, 0, 0, 0)
    speaking = asyncio.create_task(call.on_group_utterance(utterance))
    await entered.wait()
    if change == "close":
        await call.finish("alone")
    else:
        await call.switch_mode("duplex")
        if change == "mode_cycle":
            await call.switch_mode("group")
    if stage == "lock":
        call._speak_lock.release()
    release.set()
    await speaking
    assert generated == []


@pytest.mark.parametrize("change", ["close", "uncertain_mute"])
@pytest.mark.asyncio
async def test_group_greeting_rechecks_speech_admission_after_waiting_for_silence(change):
    call, _calls, _shutdowns = make_call()
    await call.set_muted(False)
    await call.switch_mode("group")
    entered = asyncio.Event()
    release = asyncio.Event()
    generated = []

    async def silence(*args):
        entered.set()
        await release.wait()

    call.wait_for_silence = silence
    call.session.generate_reply = lambda **kwargs: generated.append(kwargs)
    greeting = asyncio.create_task(call.greet())
    await entered.wait()
    if change == "close":
        await call.finish("alone")
    else:
        call.session.output.fail_once = True
        await call.set_muted(True)
    release.set()
    await greeting
    assert generated == []


class Camera(Resource):
    def __init__(self, *, fail_once=False, paused=False):
        super().__init__(fail_once=fail_once)
        self.frames_sent = 0
        self.looks = 0
        self.frame_started = asyncio.Event()
        self.frame_release = asyncio.Event()
        if not paused:
            self.frame_release.set()

    async def look(self, identity):
        self.looks += 1
        self.frame_started.set()
        await self.frame_release.wait()
        return b"synthetic frame bytes", "u_first"

    def camera_seconds(self):
        return 0

    def summary(self):
        return {}


@pytest.mark.parametrize("stage,hand_to_agent", [("admission", False), ("admission", True), ("capture", False), ("capture", True), ("upload", True)])
@pytest.mark.asyncio
async def test_look_cannot_start_new_capture_blob_or_tool_work_after_termination(stage, hand_to_agent):
    call, _calls, _shutdowns = make_call()
    camera = call.vision = Camera(fail_once=stage == "admission", paused=stage == "capture")
    uploads = []
    tools = []
    uploading = asyncio.Event()
    release_upload = asyncio.Event()

    async def upload(*args):
        uploads.append(args)
        uploading.set()
        if stage == "upload":
            await release_upload.wait()

    async def tool(*args, **kwargs):
        tools.append(args)
        return {"ok": True, "text": "synthetic"}

    call.relay.put_blob = upload
    call.relay.tool = tool
    if stage == "admission":
        await call.finish("alone")
        result = await call.look("synthetic", hand_to_agent=hand_to_agent)
        assert camera.looks == 0
    else:
        looking = asyncio.create_task(call.look("synthetic", hand_to_agent=hand_to_agent))
        await (uploading if stage == "upload" else camera.frame_started).wait()
        await call.finish("alone")
        camera.frame_release.set()
        release_upload.set()
        result = await looking
    assert "ending" in result
    assert len(uploads) == (1 if stage == "upload" else 0), "an already-dispatched upload is not recalled"
    assert tools == []


@pytest.mark.asyncio
async def test_late_tool_result_cannot_spawn_new_owned_work_after_shutdown_drain():
    call, _calls, _shutdowns = make_call()
    call.cfg = replace(call.cfg, tool_quick_wait_s=10)
    started = asyncio.Event()

    async def tool(*args, **kwargs):
        started.set()
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            return {"ok": True, "text": "synthetic late result", "message_id": "synthetic"}

    call.relay.tool = tool
    asking = asyncio.create_task(call.ask_agent("synthetic request"))
    await started.wait()
    await call.finish("alone")
    assert "ending" in await asking
    assert not call._tasks and not call._pending_tools


@pytest.mark.asyncio
async def test_terminated_call_cannot_send_new_model_context():
    call, _calls, _shutdowns = make_call()
    sent = []
    call.agent.realtime_llm_session.send_event = sent.append
    await call.finish("alone")
    call.add_context("synthetic late chat context")
    assert sent == []


@pytest.mark.asyncio
async def test_queued_tool_dispatch_rechecks_termination_before_calling_the_relay():
    call, _calls, _shutdowns = make_call()
    dispatched = []

    async def tool(*args, **kwargs):
        dispatched.append(True)
        return {"ok": True, "text": "synthetic", "message_id": "synthetic"}

    call.relay.tool = tool
    asking = asyncio.create_task(call.ask_agent("synthetic request"))
    closing = asyncio.create_task(call.finish("alone"))
    result, _ = await asyncio.gather(asking, closing)
    assert "ending" in result
    assert dispatched == []


@pytest.mark.parametrize("uncertain", [False, True])
@pytest.mark.asyncio
async def test_group_model_control_remains_available_for_stable_and_uncertain_mute_recovery(uncertain):
    from arbos_voice.judge import Decision
    from arbos_voice.listener import Utterance

    call, _calls, _shutdowns = make_call()
    await call.set_muted(False)
    await call.switch_mode("group")
    call.session.output.fail_once = uncertain
    await call.set_muted(True)
    generated = []
    call.session.generate_reply = lambda **kwargs: generated.append(kwargs)

    async def decide(*args):
        return Decision(action="act", forced=True)

    async def silence(*args):
        pass

    call.judge = SimpleNamespace(decide=decide)
    call.wait_for_silence = silence
    await call.on_group_utterance(Utterance("u_first", "unmute yourself", 0.5, 0, 0, 0))
    assert len(generated) == 1, "the existing interactive model/tool path must remain available"
    assert "Already" not in await call.set_muted(False)
    assert call.session.output.enabled is True


@pytest.mark.parametrize("mode", ["duplex", "group"])
@pytest.mark.parametrize("action", ["say", "greet"])
@pytest.mark.asyncio
async def test_running_confirmed_unmuted_call_still_generates_normal_speech(mode, action):
    call, _calls, _shutdowns = make_call()
    await call.set_muted(False)
    await call.switch_mode(mode)
    call.state.last_human_speech_at = time.monotonic() - call.cfg.group_silence_s - 1
    generated = []

    async def playout():
        pass

    def reply(**kwargs):
        generated.append(kwargs)
        return SimpleNamespace(wait_for_playout=playout)

    call.session.generate_reply = reply
    if action == "say":
        await call.say("synthetic update")
    else:
        await call.greet()
    assert len(generated) == 1
