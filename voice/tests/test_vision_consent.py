"""Consent changes invalidate an in-flight still before it can reach a model or caller."""

from __future__ import annotations

import asyncio
import json
from types import SimpleNamespace
import threading
import time

import pytest

from arbos_voice.vision import Camera, Vision
from arbos_voice import vision as vision_module


class FakeStream:
    def __init__(self):
        self.frames = asyncio.Queue()

    def __aiter__(self):
        return self

    async def __anext__(self):
        return await self.frames.get()

    async def aclose(self):
        pass


class FakeLocalParticipant:
    def __init__(self):
        self.data = []
        self.attributes = []

    async def publish_data(self, *args, **kwargs):
        self.data.append((args, kwargs))

    async def set_attributes(self, *args, **kwargs):
        self.attributes.append((args, kwargs))


class FakeRealtime:
    def __init__(self):
        self.events = []

    def send_event(self, event):
        self.events.append(event)


def make_vision():
    room = SimpleNamespace(remote_participants={}, local_participant=FakeLocalParticipant())
    cfg = SimpleNamespace(vision_fps=1.0, vision_long_edge=512, vision_look_long_edge=1280, vision_keep=4)
    vision = Vision(room, cfg, agent_identity="a_synthetic", agent_name="Test", label_of=lambda _: "Owner")
    cam = Camera(identity="u_synthetic", source="camera", sid="cam_synthetic", track=object(), pub=object())
    cam.stream = FakeStream()
    cam.latest = object()
    cam.latest_at = time.monotonic()
    vision.cameras[cam.sid] = cam
    vision.rt = FakeRealtime()
    return vision, cam


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["disable", "close", "disable-enable", "session-replaced", "detach", "mute-unmute"])
async def test_revoked_consent_rejects_a_paused_encoder_result(monkeypatch, change):
    vision, cam = make_vision()
    entered = asyncio.Event()
    release = threading.Event()
    loop = asyncio.get_running_loop()

    def paused_encode(frame, long_edge, quality=70):
        loop.call_soon_threadsafe(entered.set)
        if not release.wait(3):
            raise AssertionError("test encoder was not released")
        return b"synthetic-jpeg", object()

    monkeypatch.setattr(vision, "_encode", paused_encode)
    task = asyncio.create_task(vision.look())
    try:
        await asyncio.wait_for(entered.wait(), timeout=3)
        if change == "close":
            await vision.stop()
        elif change == "session-replaced":
            vision.rt = FakeRealtime()
        elif change == "detach":
            await vision.detach(cam.sid)
        elif change == "mute-unmute":
            monkeypatch.setattr("arbos_voice.vision.rtc.VideoStream", lambda _: FakeStream())
            pub = SimpleNamespace(sid=cam.sid)
            vision.muted(pub, None)
            await asyncio.sleep(0)
            vision.unmuted(pub, None)
        else:
            vision.set_enabled(False)
            if change == "disable-enable":
                vision.set_enabled(True)
        release.set()
        result = await asyncio.wait_for(task, timeout=3)
        assert result is None, "revoked bytes must not escape to the caller for relay upload"
        assert vision.rt.events == [], "revoked bytes must not enter model context"
        assert vision.sent == []
        assert (vision.frames_sent, vision.looks, vision.bytes_sent) == (0, 0, 0)
    finally:
        release.set()
        await task
        await vision.stop()


@pytest.mark.asyncio
@pytest.mark.parametrize("model_attached", [False, True])
async def test_current_consent_returns_a_frame_and_preserves_accounting(monkeypatch, model_attached):
    vision, cam = make_vision()
    rt = vision.rt
    if not model_attached:
        vision.rt = None
    encoded = []

    def encode(frame, long_edge, quality=70):
        encoded.append((frame, long_edge, quality))
        return b"synthetic-jpeg", object()

    monkeypatch.setattr(vision, "_encode", encode)
    try:
        assert await vision.look() == (b"synthetic-jpeg", cam.identity)
        assert encoded == [(cam.latest, 1280, 85)]
        assert vision.looks == 1
        assert vision.frames_sent == int(model_attached)
        assert vision.bytes_sent == (len(b"synthetic-jpeg") if model_attached else 0)
        assert len(rt.events) == int(model_attached)
        assert len(vision.sent) == int(model_attached)
    finally:
        await vision.stop()


@pytest.mark.asyncio
async def test_closed_vision_cannot_be_reenabled_or_restarted():
    vision, _ = make_vision()
    await vision.stop()
    try:
        assert not vision.enabled
        vision.set_enabled(True)
        vision.start()
        assert not vision.enabled
        assert vision.trigger() is False
        assert await vision.look() is None
        assert vision.rt.events == []
    finally:
        await vision.stop()


def attach_fake_camera(monkeypatch, vision, sid, stream, *, fail_open=False):
    class FakeTrack:
        pass

    monkeypatch.setattr(vision_module.rtc, "RemoteVideoTrack", FakeTrack)
    def open_stream(_):
        if fail_open:
            raise RuntimeError("synthetic stream failure")
        return stream

    monkeypatch.setattr(vision_module.rtc, "VideoStream", open_stream)
    pub = SimpleNamespace(
        sid=sid, kind=vision_module.rtc.TrackKind.KIND_VIDEO,
        source=vision_module.rtc.TrackSource.SOURCE_CAMERA, muted=False, subscribed=True,
    )
    participant = SimpleNamespace(identity="u_synthetic")
    vision.attach(FakeTrack(), pub, participant)
    return vision.cameras[sid]


@pytest.mark.asyncio
@pytest.mark.parametrize("wait_for_close", [False, True])
async def test_old_disable_cleanup_cannot_retire_a_replacement_camera(monkeypatch, wait_for_close):
    vision, old = make_vision()
    entered = asyncio.Event()
    release = asyncio.Event()

    class PausedClose(FakeStream):
        async def aclose(self):
            entered.set()
            await release.wait()

    old.stream = PausedClose()
    try:
        vision.set_enabled(False)
        if wait_for_close:
            await asyncio.wait_for(entered.wait(), timeout=3)
        vision.set_enabled(True)
        stream = FakeStream()
        fresh = attach_fake_camera(monkeypatch, vision, old.sid, stream)
        assert fresh is not old
        await asyncio.wait_for(entered.wait(), timeout=3)
        frame = object()
        stream.frames.put_nowait(SimpleNamespace(frame=frame))
        await asyncio.sleep(0)
        release.set()
        await asyncio.sleep(0)
        assert vision.cameras[old.sid] is fresh
        assert fresh.stream is stream and fresh.latest is frame
        assert old.stream is None and old.latest is None
        monkeypatch.setattr(vision, "_encode", lambda *args: (b"fresh-jpeg", object()))
        assert await vision.look() == (b"fresh-jpeg", fresh.identity)
        assert vision.frames_sent == 1
        assert vision.seeing() == [fresh.identity]
    finally:
        release.set()
        await vision.stop()


@pytest.mark.asyncio
async def test_a_cancelled_reader_cannot_restore_a_frame_after_disable(monkeypatch):
    vision, old = make_vision()
    await vision.detach(old.sid)
    entered = asyncio.Event()
    release = asyncio.Event()
    observed = []
    vision.on_first_frame = observed.append

    class LateFrame(FakeStream):
        def __init__(self):
            super().__init__()
            self.emitted = False

        async def __anext__(self):
            if self.emitted:
                raise StopAsyncIteration
            entered.set()
            try:
                await release.wait()
            except asyncio.CancelledError:
                # A frame already in an SDK completion can outlive cancellation.
                await release.wait()
            self.emitted = True
            return SimpleNamespace(frame=object())

    try:
        cam = attach_fake_camera(monkeypatch, vision, old.sid, LateFrame())
        await asyncio.wait_for(entered.wait(), timeout=3)
        vision.set_enabled(False)
        release.set()
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        assert cam.latest is None
        assert observed == []
        assert vision.seeing() == []
        assert vision.trigger() is False
        assert await vision.look() is None
    finally:
        release.set()
        await vision.stop()


@pytest.mark.asyncio
async def test_close_revokes_observable_seeing_before_sdk_cleanup_finishes(monkeypatch):
    vision, old = make_vision()
    await vision.detach(old.sid)
    entered = asyncio.Event()
    release = asyncio.Event()

    class PausedClose(FakeStream):
        async def aclose(self):
            entered.set()
            await release.wait()

    attach_fake_camera(monkeypatch, vision, old.sid, PausedClose())
    assert vision.seeing() == [old.identity]
    task = asyncio.create_task(vision.stop())
    try:
        await asyncio.wait_for(entered.wait(), timeout=3)
        assert not vision.enabled
        assert vision.seeing() == []
        assert vision.trigger() is False
        assert await vision.look() is None
    finally:
        release.set()
        await task


@pytest.mark.asyncio
async def test_camera_time_is_folded_once_before_paused_cleanup(monkeypatch):
    vision, old = make_vision()
    now = [10.0]
    # Replace this module's clock binding, leaving asyncio's real monotonic clock alone.
    monkeypatch.setattr(vision_module, "time", SimpleNamespace(monotonic=lambda: now[0]))

    class PausedClose(FakeStream):
        def __init__(self):
            super().__init__()
            self.entered = asyncio.Event()
            self.release = asyncio.Event()

        async def aclose(self):
            self.entered.set()
            await self.release.wait()

    old.started_at = 3.0
    old_stream = PausedClose()
    fresh_stream = PausedClose()
    old.stream = old_stream
    task = None
    try:
        assert vision.camera_seconds() == 7.0
        vision.set_enabled(False)
        assert vision.camera_seconds() == 7.0
        assert vision.summary()["camera_seconds"] == 7.0
        await asyncio.wait_for(old_stream.entered.wait(), timeout=3)
        now[0] = 12.0
        vision.set_enabled(True)
        attach_fake_camera(monkeypatch, vision, old.sid, fresh_stream)
        now[0] = 15.0
        assert vision.camera_seconds() == 10.0
        task = asyncio.create_task(vision.stop())
        await asyncio.wait_for(fresh_stream.entered.wait(), timeout=3)
        assert vision.camera_seconds() == 10.0
        assert vision.summary()["camera_seconds"] == 10.0
        now[0] = 20.0
        assert vision.camera_seconds() == 10.0
        old_stream.release.set()
        fresh_stream.release.set()
        await task
        await vision.stop()
        vision.set_enabled(True)
        assert vision.camera_seconds() == 10.0
    finally:
        old_stream.release.set()
        fresh_stream.release.set()
        if task is not None:
            await task
        await vision.stop()


@pytest.mark.asyncio
async def test_immediate_unmute_keeps_the_new_stream_while_old_mute_cleanup_waits(monkeypatch):
    vision, cam = make_vision()
    entered = asyncio.Event()
    release = asyncio.Event()

    class PausedClose(FakeStream):
        async def aclose(self):
            entered.set()
            await release.wait()

    cam.stream = PausedClose()
    fresh_stream = FakeStream()
    monkeypatch.setattr(vision_module.rtc, "VideoStream", lambda _: fresh_stream)
    pub = SimpleNamespace(sid=cam.sid)
    try:
        vision.muted(pub, None)
        vision.unmuted(pub, None)
        await asyncio.wait_for(entered.wait(), timeout=3)
        assert cam.stream is fresh_stream
        frame = object()
        fresh_stream.frames.put_nowait(SimpleNamespace(frame=frame))
        await asyncio.sleep(0)
        release.set()
        await asyncio.sleep(0)
        assert cam.stream is fresh_stream and cam.latest is frame
        assert vision.seeing() == [cam.identity]
    finally:
        release.set()
        await vision.stop()


@pytest.mark.asyncio
async def test_failed_stream_open_can_be_retried_for_the_same_publication(monkeypatch):
    vision, old = make_vision()
    await vision.detach(old.sid)
    stream = FakeStream()
    try:
        with pytest.raises(RuntimeError, match="synthetic stream failure"):
            attach_fake_camera(monkeypatch, vision, old.sid, stream, fail_open=True)
        assert vision.cameras == {}
        cam = attach_fake_camera(monkeypatch, vision, old.sid, stream)
        assert cam.stream is stream
    finally:
        await vision.stop()


@pytest.mark.asyncio
async def test_muting_clears_seeing_before_paused_sdk_cleanup(monkeypatch):
    vision, old = make_vision()
    await vision.detach(old.sid)
    entered = asyncio.Event()
    release = asyncio.Event()

    class PausedClose(FakeStream):
        async def aclose(self):
            entered.set()
            await release.wait()

    cam = attach_fake_camera(monkeypatch, vision, old.sid, PausedClose())
    pub = SimpleNamespace(sid=cam.sid)
    try:
        assert vision.seeing() == [cam.identity]
        vision.muted(pub, None)
        assert vision.seeing() == []
        await asyncio.wait_for(entered.wait(), timeout=3)
        assert vision.seeing() == []
    finally:
        release.set()
        await vision.stop()


@pytest.mark.asyncio
async def test_a_queued_off_status_cannot_overwrite_a_fresh_seeing_attribute(monkeypatch):
    vision, cam = make_vision()
    entered = asyncio.Event()
    release = asyncio.Event()

    class PausedOffStatus(FakeLocalParticipant):
        async def publish_data(self, payload, **kwargs):
            self.data.append(payload)
            if not json.loads(payload)["seeing"]:
                entered.set()
                await release.wait()

    local = PausedOffStatus()
    vision.room.local_participant = local
    monkeypatch.setattr(vision_module.rtc, "VideoStream", lambda _: FakeStream())
    pub = SimpleNamespace(sid=cam.sid)
    try:
        vision.muted(pub, None)
        await asyncio.wait_for(entered.wait(), timeout=3)
        vision.unmuted(pub, None)
        await asyncio.sleep(0)
        release.set()
        await asyncio.sleep(0)
        snapshots = [json.loads(args[0]["agent_vision"]) for args, _ in local.attributes]
        assert len(snapshots) == 1
        assert snapshots[0]["seeing"] is True
        assert snapshots[0]["seeing_identities"] == [cam.identity]
        assert vision.seeing() == [cam.identity]
    finally:
        release.set()
        await vision.stop()


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["stop", "detach"])
async def test_cancelled_stop_keeps_all_stream_cleanup_owned_for_retry(operation):
    vision, first = make_vision()

    class PausedClose(FakeStream):
        def __init__(self):
            super().__init__()
            self.entered = asyncio.Event()
            self.release = asyncio.Event()
            self.calls = 0
            self.cancelled = False

        async def aclose(self):
            self.calls += 1
            self.entered.set()
            try:
                await self.release.wait()
            except asyncio.CancelledError:
                self.cancelled = True
                raise

    streams = [PausedClose(), PausedClose()]
    first.stream = streams[0]
    second = Camera(identity="u_second", source="screen", sid="cam_second", track=object(), pub=object())
    second.stream = streams[1]
    vision.cameras[second.sid] = second
    task = asyncio.create_task(vision.stop() if operation == "stop" else vision.detach(first.sid))
    retry = None
    try:
        await asyncio.wait_for(streams[0].entered.wait(), timeout=3)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        retry = asyncio.create_task(vision.stop())
        await asyncio.sleep(0)
        assert not retry.done(), "retry must join unfinished SDK cleanup after cameras are retired"
        await asyncio.wait_for(asyncio.gather(*(stream.entered.wait() for stream in streams)), timeout=3)
        assert all(not stream.cancelled for stream in streams)
        for stream in streams:
            stream.release.set()
        await asyncio.wait_for(retry, timeout=3)
        assert [stream.calls for stream in streams] == [1, 1]
        assert not vision.enabled and vision.seeing() == []
    finally:
        for stream in streams:
            stream.release.set()
        if retry is not None:
            await retry
        await vision.stop()


@pytest.mark.asyncio
async def test_sdk_cleanup_error_is_best_effort_and_logs_no_raw_diagnostic(caplog):
    vision, cam = make_vision()

    class FailedClose(FakeStream):
        async def aclose(self):
            raise RuntimeError("synthetic-private-sdk-diagnostic")

    cam.stream = FailedClose()
    with caplog.at_level("WARNING", logger="arbos-voice.vision"):
        await vision.stop()
    assert "RuntimeError" in caplog.text
    assert "synthetic-private-sdk-diagnostic" not in caplog.text
    assert not vision.enabled and vision.seeing() == []
