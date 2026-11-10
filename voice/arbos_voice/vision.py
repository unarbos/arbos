"""Camera vision on agent calls: the agent sees what the humans point their camera at.

While a human's camera (or screen share) is on and the owner has not turned vision off, the
worker subscribes to that video track and feeds the speech model **stills, never video**:

* a downscaled JPEG (long edge `vision_long_edge`, default 512 px, `detail: low`) about once a
  second while the picture changes; a still picture is re-sent only every `VISION_STATIC_EVERY_S`;
  a sudden scene change (frame difference above `SCENE_CHANGE_DIFF`) sends one at once;
* an extra frame when someone says "look at this" / "can you see" (the worker calls `trigger`);
* on demand, `look()` captures a full-resolution frame (long edge `vision_look_long_edge`,
  `detail: high`) for labels and screens, and returns the JPEG bytes so the worker can also hand
  it to the full agent through the relay.

Old frames are deleted from the model's conversation (`conversation.item.delete`) so that at most
`vision_keep` recent frames plus the latest one per participant stay in context: cost and
context stay bounded. In group calls every frame is labelled with who it came from.

Clients learn when the agent is looking from a reliable LiveKit data message on topic
`agent_vision`, sent by the agent participant once per participant at every start and stop of
sampling: `{"type":"agent_vision","participant_identity":"u_<uid>","seeing":true|false,
"agent_name":"Forest","agent":"a_<pid>","seeing_identities":[…]}`. The same JSON is mirrored into
the agent participant's attribute `agent_vision` (last event; `seeing_identities` is the full
state for late joiners). Camera off (mute / unpublish) stops the frames at once and clears it.
"""

from __future__ import annotations

import asyncio
import base64
import io
import json
import logging
import time
import uuid
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

import numpy as np
from livekit import rtc
from livekit.agents.utils.images import EncodeOptions, ResizeOptions, encode
from PIL import Image

from .config import Config
from .vision_state import Closed, Disabled, Enabled, VisionState, accepts_frame, capture_generation, transition

log = logging.getLogger("arbos-voice.vision")

VISION_TOPIC = "agent_vision"
VISION_ATTR = "agent_vision"
# A still picture (difference below this, 0–255 grey scale) is not re-sent every second.
STATIC_DIFF = 2.5
# ... but at least this often, so the model never works from a stale frame.
VISION_STATIC_EVERY_S = 5.0
# A difference above this is a new scene: send a frame at once (after `MIN_GAP_S`). A hand-held
# phone camera moves all the time, so this gate is high and the gap caps motion at 2 frames/s.
SCENE_CHANGE_DIFF = 30.0
MIN_GAP_S = 0.5
THUMB = (32, 18)
# Phrases that mean "use your eyes now" (checked by the worker on each transcript line).
LOOK_PHRASES = (
    "look at this", "look at that", "look here", "can you see", "do you see", "what do you see",
    "what is this", "what's this", "what am i holding", "see this", "take a look", "have a look",
    "read this", "read the label", "what does this say", "describe this", "check this out",
    "what's written", "what is written", "what does it say", "printed on", "written on",
)

SOURCES = {
    rtc.TrackSource.SOURCE_CAMERA: "camera",
    rtc.TrackSource.SOURCE_SCREENSHARE: "screen",
}


def wants_a_look(text: str) -> bool:
    t = " ".join("".join(c.lower() if c.isalnum() or c == "'" else " " for c in text).split())
    return any(p in t for p in LOOK_PHRASES)


def clock() -> str:
    return time.strftime("%H:%M:%S", time.gmtime())


@dataclass
class Camera:
    identity: str
    source: str
    sid: str
    track: rtc.RemoteVideoTrack
    pub: rtc.RemoteTrackPublication
    started_at: float = field(default_factory=time.monotonic)
    stream: rtc.VideoStream | None = None
    task: asyncio.Task | None = None
    latest: rtc.VideoFrame | None = None
    latest_at: float = 0.0
    last_sent_at: float = 0.0
    last_thumb: np.ndarray | None = None
    frames_sent: int = 0


@dataclass
class SentFrame:
    item_id: str
    identity: str
    at: float
    look: bool = False


@dataclass(frozen=True)
class _FrameLease:
    generation: int
    stream: rtc.VideoStream
    rt: Any


class Vision:
    """One per call. `rt` (the realtime session) is set once the agent session is up."""

    def __init__(
        self,
        room: rtc.Room,
        cfg: Config,
        *,
        agent_identity: str,
        agent_name: str,
        label_of: Callable[[str], str],
        enabled: bool = True,
    ):
        self.room = room
        self.cfg = cfg
        self.agent_identity = agent_identity
        self.agent_name = agent_name
        self.label_of = label_of
        self._state: VisionState = Enabled() if enabled else Disabled()
        self.rt: Any = None
        self.local_ctx: Callable[[], Any] | None = None
        self.cameras: dict[str, Camera] = {}
        self.sent: list[SentFrame] = []
        self.frames_sent = 0
        self.looks = 0
        self.bytes_sent = 0
        self._camera_seconds = 0.0
        self._task: asyncio.Task | None = None
        self._closing_tasks: set[asyncio.Task[None]] = set()
        self.on_first_frame: Callable[[str], None] | None = None

    # ---- lifecycle -------------------------------------------------------------------------------

    @property
    def enabled(self) -> bool:
        return isinstance(self._state, Enabled)

    def start(self) -> None:
        if isinstance(self._state, Closed):
            return
        if self._task is None:
            self._task = asyncio.create_task(self._sampler())
        for p in self.room.remote_participants.values():
            for pub in p.track_publications.values():
                self.consider(pub, p)

    async def stop(self) -> None:
        self._state = transition(self._state, "close")
        if self._task:
            self._task.cancel()
        for cam, stream in self._retire_cameras():
            if stream is not None:
                self._schedule_close(cam, stream)
        if self._closing_tasks:
            # Caller deadlines must not abandon native streams or cancel another stop's cleanup.
            await asyncio.shield(asyncio.gather(*tuple(self._closing_tasks)))

    def set_enabled(self, enabled: bool) -> None:
        state = transition(self._state, "enable" if enabled else "disable")
        if state is self._state:
            return
        self._state = state
        log.info("vision %s", "on" if enabled else "off")
        if enabled:
            for p in self.room.remote_participants.values():
                for pub in p.track_publications.values():
                    self.consider(pub, p)
        else:
            # Retire old camera ownership synchronously. A re-subscription may attach a
            # replacement before SDK close completes; that cleanup must own only its old stream.
            closing = self._retire_cameras()
            for p in self.room.remote_participants.values():
                for pub in p.track_publications.values():
                    if pub.kind == rtc.TrackKind.KIND_VIDEO and pub.subscribed:
                        pub.set_subscribed(False)
            for identity in {cam.identity for cam, _ in closing}:
                self._signal(identity, False)
                self._forget(identity, keep=1)
            for cam, stream in closing:
                if stream is not None:
                    self._schedule_close(cam, stream)

    # ---- tracks ------------------------------------------------------------------------------------

    @staticmethod
    def is_vision_track(pub: rtc.RemoteTrackPublication, participant: rtc.RemoteParticipant) -> bool:
        return (
            pub.kind == rtc.TrackKind.KIND_VIDEO
            and pub.source in SOURCES
            and participant.identity.startswith("u_")
        )

    def consider(self, pub: rtc.RemoteTrackPublication, participant: rtc.RemoteParticipant) -> None:
        """A video publication appeared (or vision was switched on): subscribe when we may look."""
        if not self.is_vision_track(pub, participant):
            return
        if self.enabled and not pub.subscribed:
            pub.set_subscribed(True)
        elif not self.enabled and pub.subscribed:
            pub.set_subscribed(False)

    def attach(self, track: rtc.Track, pub: rtc.RemoteTrackPublication, participant: rtc.RemoteParticipant) -> None:
        if not self.is_vision_track(pub, participant) or not isinstance(track, rtc.RemoteVideoTrack):
            return
        if not self.enabled:
            pub.set_subscribed(False)
            return
        key = pub.sid
        if key in self.cameras:
            return
        cam = Camera(identity=participant.identity, source=SOURCES.get(pub.source, "camera"), sid=key, track=track, pub=pub)
        if pub.muted:
            # Published but off: keep the subscription, wait for unmute.
            self.cameras[key] = cam
            return
        self.cameras[key] = cam
        try:
            self._open(cam)
        except Exception:
            if self.cameras.get(key) is cam:
                del self.cameras[key]
            raise

    def _open(self, cam: Camera) -> None:
        generation = capture_generation(self._state)
        if generation is None or self.cameras.get(cam.sid) is not cam or cam.stream is not None:
            return
        cam.stream = rtc.VideoStream(cam.track)
        cam.task = asyncio.create_task(self._read(cam, cam.stream, generation))
        cam.started_at = time.monotonic()
        cam.last_thumb = None
        log.info("%s on: %s (%s)", cam.source, cam.identity, cam.sid)
        self._signal(cam.identity, True)

    def _begin_close(self, cam: Camera) -> rtc.VideoStream | None:
        stream = cam.stream
        if stream is None:
            return None
        self._camera_seconds += max(0.0, time.monotonic() - cam.started_at)
        if cam.task:
            cam.task.cancel()
            cam.task = None
        cam.stream = None
        cam.latest = None
        return stream

    def _retire_cameras(self) -> list[tuple[Camera, rtc.VideoStream | None]]:
        closing = [(cam, self._begin_close(cam)) for cam in self.cameras.values()]
        self.cameras.clear()
        return closing

    async def _finish_close(self, cam: Camera, stream: rtc.VideoStream) -> None:
        try:
            await stream.aclose()
        except Exception as error:
            # SDK aclose disposes the native handle before awaiting its frame pump. Keep
            # ordinary cleanup failures best-effort, without logging raw SDK diagnostics.
            log.warning("video stream close failed (%s)", type(error).__name__)
        log.info("%s off: %s (%s)", cam.source, cam.identity, cam.sid)

    def _schedule_close(self, cam: Camera, stream: rtc.VideoStream) -> asyncio.Task[None]:
        task = asyncio.create_task(self._finish_close(cam, stream))
        self._closing_tasks.add(task)
        task.add_done_callback(self._closing_tasks.discard)
        return task

    async def detach(self, sid: str, *, signal: bool = True) -> None:
        cam = self.cameras.pop(sid, None)
        if cam is None:
            return
        stream = self._begin_close(cam)
        if signal and not any(c.identity == cam.identity and c.stream for c in self.cameras.values()):
            self._signal(cam.identity, False)
            self._forget(cam.identity, keep=1)
        if stream is not None:
            await asyncio.shield(self._schedule_close(cam, stream))

    def muted(self, pub: rtc.RemoteTrackPublication, participant: rtc.RemoteParticipant) -> None:
        cam = self.cameras.get(pub.sid)
        if cam is None:
            return
        stream = self._begin_close(cam)
        if not any(c.identity == cam.identity and c.stream for c in self.cameras.values()):
            self._signal(cam.identity, False)
            self._forget(cam.identity, keep=1)
        if stream is not None:
            self._schedule_close(cam, stream)

    def unmuted(self, pub: rtc.RemoteTrackPublication, participant: rtc.RemoteParticipant) -> None:
        cam = self.cameras.get(pub.sid)
        if cam is None or not self.enabled:
            return
        self._open(cam)

    async def _read(self, cam: Camera, stream: rtc.VideoStream, generation: int) -> None:
        first = True
        try:
            async for ev in stream:
                if not accepts_frame(self._state, generation) or self.cameras.get(cam.sid) is not cam or cam.stream is not stream:
                    break
                cam.latest = ev.frame
                cam.latest_at = time.monotonic()
                if first:
                    first = False
                    if self.on_first_frame:
                        self.on_first_frame(cam.identity)
        except asyncio.CancelledError:
            pass
        except Exception as e:
            log.warning("video stream %s ended: %s", cam.sid, e)

    # ---- state -------------------------------------------------------------------------------------

    def seeing(self) -> list[str]:
        return sorted({c.identity for c in self.live_cameras()}) if self.enabled else []

    def live_cameras(self) -> list[Camera]:
        return [c for c in self.cameras.values() if c.stream is not None]

    def camera_seconds(self) -> float:
        now = time.monotonic()
        return self._camera_seconds + sum(max(0.0, now - c.started_at) for c in self.live_cameras())

    def _signal(self, identity: str, seeing: bool) -> None:
        if isinstance(self._state, Closed) or (seeing and not self.enabled):
            return
        generation = self._state.generation
        snapshot = self.seeing()
        seeing = identity in snapshot
        lp = self.room.local_participant
        payload = json.dumps(
            {
                "type": "agent_vision",
                "participant_identity": identity,
                "seeing": seeing,
                "agent_name": self.agent_name,
                "agent": self.agent_identity,
                "seeing_identities": snapshot,
            }
        )

        def current() -> bool:
            return self._state.generation == generation and not isinstance(self._state, Closed) and self.seeing() == snapshot

        async def send() -> None:
            if not current():
                return
            try:
                await lp.publish_data(payload, reliable=True, topic=VISION_TOPIC)
            except Exception as e:
                log.warning("publish_data failed: %s", e)
            if not current():
                return
            try:
                await lp.set_attributes({VISION_ATTR: payload})
            except Exception as e:
                log.warning("set_attributes failed: %s", e)

        asyncio.create_task(send())

    # ---- frames ------------------------------------------------------------------------------------

    def _capture(self, cam: Camera) -> _FrameLease | None:
        generation = capture_generation(self._state)
        if generation is None or self.cameras.get(cam.sid) is not cam or cam.stream is None:
            return None
        return _FrameLease(generation, cam.stream, self.rt)

    def _current(self, cam: Camera, lease: _FrameLease) -> bool:
        return (
            accepts_frame(self._state, lease.generation)
            and self.cameras.get(cam.sid) is cam
            and cam.stream is lease.stream
            and self.rt is lease.rt
        )

    async def _sampler(self) -> None:
        period = 1.0 / max(0.2, self.cfg.vision_fps)
        while not isinstance(self._state, Closed):
            await asyncio.sleep(0.2)
            if not self.enabled or self.rt is None:
                continue
            now = time.monotonic()
            for cam in self.live_cameras():
                if cam.latest is None or cam.latest_at <= cam.last_sent_at:
                    continue
                since = now - cam.last_sent_at
                if since < MIN_GAP_S:
                    continue
                lease = self._capture(cam)
                if lease is None:
                    continue
                try:
                    jpeg, thumb = self._encode(cam.latest, self.cfg.vision_long_edge)
                except Exception as e:
                    log.warning("encode failed: %s", e)
                    continue
                diff = self._diff(cam.last_thumb, thumb)
                if since >= period and (diff is None or diff >= STATIC_DIFF or since >= VISION_STATIC_EVERY_S):
                    pass
                elif diff is not None and diff >= SCENE_CHANGE_DIFF:
                    pass
                else:
                    continue
                self._send(cam, jpeg, thumb, lease=lease, detail="low", look=False, why=f"scene change {diff:.0f}" if (diff or 0) >= SCENE_CHANGE_DIFF else "")

    def trigger(self, identity: str | None = None) -> bool:
        """Someone said "look at this": send the newest frame now (from them, else from anyone)."""
        if not self.enabled:
            return False
        cams = [c for c in self.live_cameras() if c.latest is not None and (identity is None or c.identity == identity)]
        if not cams and identity is not None:
            cams = [c for c in self.live_cameras() if c.latest is not None]
        if not cams or self.rt is None:
            return False
        cam = max(cams, key=lambda c: c.latest_at)
        lease = self._capture(cam)
        if lease is None:
            return False
        try:
            jpeg, thumb = self._encode(cam.latest, self.cfg.vision_long_edge)
        except Exception as e:
            log.warning("encode failed: %s", e)
            return False
        return self._send(cam, jpeg, thumb, lease=lease, detail="low", look=False, why="asked to look") is not None

    async def look(self, identity: str | None = None) -> tuple[bytes, str] | None:
        """Full-resolution frame for the `look` tool → (jpeg, identity). None when no camera is on."""
        if not self.enabled:
            return None
        cams = [c for c in self.live_cameras() if c.latest is not None and (identity is None or c.identity == identity)]
        if not cams and identity is not None:
            cams = [c for c in self.live_cameras() if c.latest is not None]
        if not cams:
            return None
        cam = max(cams, key=lambda c: c.latest_at)
        lease = self._capture(cam)
        if lease is None:
            return None
        jpeg, thumb = await asyncio.get_running_loop().run_in_executor(
            None, self._encode, cam.latest, self.cfg.vision_look_long_edge, 85
        )
        if not self._current(cam, lease):
            return None
        if lease.rt is not None and self._send(cam, jpeg, thumb, lease=lease, detail="high", look=True, why="look") is None:
            return None
        self.looks += 1
        if not self._current(cam, lease):
            return None
        return jpeg, cam.identity

    def _encode(self, frame: rtc.VideoFrame, long_edge: int, quality: int = 70) -> tuple[bytes, np.ndarray]:
        w, h = frame.width, frame.height
        scale = min(1.0, long_edge / max(w, h, 1))
        opts = EncodeOptions(
            format="JPEG",
            quality=quality,
            resize_options=ResizeOptions(width=max(1, int(w * scale)), height=max(1, int(h * scale)), strategy="scale_aspect_fit"),
        )
        jpeg = encode(frame, opts)
        thumb = np.asarray(Image.open(io.BytesIO(jpeg)).convert("L").resize(THUMB), dtype=np.float32)
        return jpeg, thumb

    @staticmethod
    def _diff(a: np.ndarray | None, b: np.ndarray) -> float | None:
        if a is None:
            return None
        return float(np.mean(np.abs(a - b)))

    def _send(self, cam: Camera, jpeg: bytes, thumb: np.ndarray, *, lease: _FrameLease, detail: str, look: bool, why: str) -> str | None:
        if not self._current(cam, lease) or lease.rt is None:
            return None
        item_id = "vis_" + uuid.uuid4().hex[:12]
        label = self.label_of(cam.identity)
        what = "screen share" if cam.source == "screen" else "camera"
        text = (
            f"Full-resolution look at {label}'s {what} ({clock()} UTC)."
            if look
            else f"{label}'s {what} right now ({clock()} UTC). Mention it only when it is relevant or asked."
        )
        if not self._current(cam, lease):
            return None
        lease.rt.send_event(
            {
                "type": "conversation.item.create",
                "item": {
                    "id": item_id,
                    "type": "message",
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": text},
                        {"type": "input_image", "image_url": "data:image/jpeg;base64," + base64.b64encode(jpeg).decode(), "detail": detail},
                    ],
                },
            }
        )
        now = time.monotonic()
        cam.last_sent_at = now
        cam.last_thumb = thumb
        cam.frames_sent += 1
        self.frames_sent += 1
        self.bytes_sent += len(jpeg)
        self.sent.append(SentFrame(item_id=item_id, identity=cam.identity, at=now, look=look))
        log.info("frame → model: %s %s %d KB detail=%s%s", label, what, len(jpeg) // 1024, detail, f" ({why})" if why else "")
        self._prune()
        return item_id

    def _prune(self) -> None:
        keep = max(1, self.cfg.vision_keep)
        if len(self.sent) <= keep:
            return
        newest_per_identity = {}
        for f in self.sent:
            newest_per_identity[f.identity] = f.item_id
        protected = set(newest_per_identity.values()) | {f.item_id for f in self.sent[-keep:]}
        drop = [f for f in self.sent if f.item_id not in protected]
        for f in drop:
            self._delete(f.item_id)
        self.sent = [f for f in self.sent if f.item_id in protected]

    def _forget(self, identity: str, *, keep: int) -> None:
        mine = [f for f in self.sent if f.identity == identity]
        for f in mine[: max(0, len(mine) - keep)]:
            self._delete(f.item_id)
            self.sent.remove(f)

    def _delete(self, item_id: str) -> None:
        if self.rt is None:
            return
        self.rt.send_event({"type": "conversation.item.delete", "item_id": item_id})
        # The framework mirrors server items into the agent's local context; drop the placeholder
        # too, or a later context sync would re-create (and re-bill) the frame.
        ctx = self.local_ctx() if self.local_ctx else None
        items = getattr(ctx, "items", None)
        if items:
            for i, it in enumerate(items):
                if getattr(it, "id", None) == item_id:
                    del items[i]
                    break

    def summary(self) -> dict:
        return {
            "frames": self.frames_sent,
            "looks": self.looks,
            "kb_sent": self.bytes_sent // 1024,
            "camera_seconds": round(self.camera_seconds(), 1),
            "cameras": len(self.live_cameras()),
            "in_context": len(self.sent),
        }
