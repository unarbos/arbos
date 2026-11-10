//! Calls v1 — the LiveKit side of a user↔user call (contract:
//! internal/calls-v1-contract.md + people-chats-api.md §8). One tokio task
//! owns the `Room`; the UI talks to it over channels, like the updater.
//!
//!   AppState ── CallCmd ──▶ run() ── CallEvent ──▶ AppState::apply_call_event
//!
//! Audio goes through WebRTC's own device module (`PlatformAudio`: mic in,
//! speakers out, echo cancellation), so nothing here touches cpal. Video in
//! arrives as I420 frames from `NativeVideoStream`, is converted to BGRA and
//! handed to GPUI as a `RenderImage` (throttled to ~15 fps per tile). Video
//! out comes from the camera (macOS: AVFoundation through `nokhwa`) as I420
//! into a `NativeVideoSource`.

use crate::call_attempt::CallAttempt;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui::RenderImage;
use livekit::options::{TrackPublishOptions, VideoCodec};
use livekit::prelude::*;
use livekit::webrtc::prelude::*;
use livekit::webrtc::video_frame::native::VideoFrameBufferExt;
use livekit::webrtc::video_source::native::NativeVideoSource;
use livekit::webrtc::video_stream::native::NativeVideoStream;
use tokio::sync::mpsc;

/// Remote tiles are redrawn at most this often (GPUI uploads a texture per frame).
const TILE_FRAME_MS: u64 = 66;
/// Remote frames are scaled down to at most this many pixels wide before upload.
const TILE_MAX_WIDTH: u32 = 960;
/// Camera publish size.
const CAMERA_WIDTH: u32 = 640;
const CAMERA_HEIGHT: u32 = 480;

#[derive(Debug)]
pub enum CallCmd {
    /// Connect to the room and publish the mic (and the camera for a video call).
    Join {
        attempt: CallAttempt,
        url: String,
        token: String,
        camera: bool,
    },
    SetMuted {
        attempt: CallAttempt,
        muted: bool,
    },
    SetCamera {
        attempt: CallAttempt,
        on: bool,
    },
    Leave {
        attempt: CallAttempt,
    },
}

#[derive(Debug, Clone)]
pub enum CallEvent {
    Connecting,
    Connected,
    /// A remote participant is in the room (`present`) or left it.
    Participant {
        identity: String,
        name: String,
        /// `sender{}` JSON the relay put on the token.
        metadata: String,
        present: bool,
    },
    /// New frame for a participant's tile (`identity` = `local` for my camera).
    Video {
        identity: String,
        frame: Arc<RenderImage>,
    },
    /// The participant's camera track went away: back to the avatar.
    VideoOff {
        identity: String,
    },
    /// Who is talking right now.
    Speaking(Vec<String>),
    /// Agent vision (voice API §11): the voice worker is (not) sampling a
    /// participant's camera. `identity` is the one the message is about;
    /// `seeing_identities` is the full set (late joiners read the attribute).
    AgentVision {
        identity: String,
        seeing: bool,
        seeing_identities: Vec<String>,
    },
    /// A remote mic was muted / unmuted.
    RemoteMuted {
        identity: String,
        muted: bool,
    },
    /// My camera could not start (no device, no permission): stays off.
    CameraUnavailable(String),
    /// My mic could not start: the call continues listen-only.
    MicUnavailable(String),
    Error(String),
    Disconnected,
}

/// The UI discards every event whose lease no longer owns its pending call.
#[derive(Debug, Clone)]
pub struct AttemptEvent {
    pub attempt: CallAttempt,
    pub event: CallEvent,
}

#[derive(Clone)]
struct CallEvents {
    attempt: CallAttempt,
    sender: mpsc::UnboundedSender<AttemptEvent>,
}

impl CallEvents {
    fn send(&self, event: CallEvent) -> Result<(), ()> {
        if !self.attempt.is_active() {
            return Ok(());
        }
        self.sender
            .send(AttemptEvent {
                attempt: self.attempt.clone(),
                event,
            })
            .map_err(|_| ())
    }
}

struct Session {
    room: Arc<Room>,
    mic: Option<LocalAudioTrack>,
    /// Keeps WebRTC's audio device module alive for the call.
    _audio: Option<PlatformAudio>,
    camera: Option<CameraPublish>,
    video_tasks: HashMap<String, tokio::task::JoinHandle<()>>,
    closed: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        for task in self.video_tasks.values() {
            task.abort();
        }
        if let Some(camera) = &self.camera {
            camera
                .stop
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if !self.closed {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let room = self.room.clone();
                runtime.spawn(async move {
                    let _ = room.close().await;
                });
            }
        }
    }
}

struct CameraPublish {
    track: LocalVideoTrack,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for CameraPublish {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The engine loop. Runs on the tokio runtime for the app's lifetime.
pub async fn run(
    mut cmds: mpsc::UnboundedReceiver<CallCmd>,
    events: mpsc::UnboundedSender<AttemptEvent>,
) {
    let mut session: Option<(CallAttempt, Session)> = None;
    let mut room_events: Option<mpsc::UnboundedReceiver<RoomEvent>> = None;
    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    CallCmd::Join { attempt, url, token, camera } => {
                        if !attempt.is_active() { continue; }
                        if let Some((_, old)) = session.take() { end_session(old).await; }
                        room_events = None;
                        if !attempt.is_active() { continue; }
                        let scoped = CallEvents { attempt: attempt.clone(), sender: events.clone() };
                        let _ = scoped.send(CallEvent::Connecting);
                        let result = tokio::select! {
                            _ = attempt.cancelled() => continue,
                            result = connect(&url, &token, camera, &scoped, &attempt) => result,
                        };
                        match result {
                            Ok((s, rx)) if attempt.is_active() => {
                                session = Some((attempt, s));
                                room_events = Some(rx);
                                let _ = scoped.send(CallEvent::Connected);
                            }
                            Ok((s, _)) => end_session(s).await,
                            Err(err) => { let _ = scoped.send(CallEvent::Error(format!("{err:#}"))); }
                        }
                    }
                    CallCmd::SetMuted { attempt, muted } => {
                        if let Some((owner, s)) = session.as_ref().filter(|(owner, _)| owner.owns(&attempt)) {
                            let _ = owner;
                            if let Some(mic) = s.mic.as_ref() { if muted { mic.mute() } else { mic.unmute() } }
                        }
                    }
                    CallCmd::SetCamera { attempt, on } => {
                        if let Some((owner, s)) = session.as_mut().filter(|(owner, _)| owner.owns(&attempt)) {
                            let scoped = CallEvents { attempt: owner.clone(), sender: events.clone() };
                            if on && s.camera.is_none() {
                                let result = tokio::select! {
                                    _ = attempt.cancelled() => continue,
                                    result = publish_camera(&s.room, &scoped) => result,
                                };
                                match result {
                                    Ok(c) if attempt.is_active() => s.camera = Some(c),
                                    Ok(c) => stop_camera(&s.room, c).await,
                                    Err(err) => { let _ = scoped.send(CallEvent::CameraUnavailable(format!("{err:#}"))); }
                                }
                            } else if !on {
                                if let Some(c) = s.camera.take() {
                                    stop_camera(&s.room, c).await;
                                    let _ = scoped.send(CallEvent::VideoOff { identity: "local".into() });
                                }
                            }
                        }
                    }
                    CallCmd::Leave { attempt } => {
                        if session.as_ref().is_some_and(|(owner, _)| owner.is_same(&attempt)) {
                            if let Some((_, old)) = session.take() { end_session(old).await; }
                            room_events = None;
                            let _ = CallEvents { attempt, sender: events.clone() }.send(CallEvent::Disconnected);
                        }
                    }
                }
            }
            _ = async {
                match session.as_ref() {
                    Some((owner, _)) => owner.cancelled().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some((_, old)) = session.take() { end_session(old).await; }
                room_events = None;
            }
            ev = async {
                match room_events.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                let Some(ev) = ev else { room_events = None; continue; };
                if let Some((owner, s)) = session.as_mut() {
                    let scoped = CallEvents { attempt: owner.clone(), sender: events.clone() };
                    if owner.is_active() { on_room_event(s, ev, &scoped); }
                }
            }
        }
    }
    if let Some((_, old)) = session {
        end_session(old).await;
    }
}

async fn connect(
    url: &str,
    token: &str,
    camera: bool,
    events: &CallEvents,
    attempt: &CallAttempt,
) -> anyhow::Result<(Session, mpsc::UnboundedReceiver<RoomEvent>)> {
    // Audio first: WebRTC's device module must exist before the peer connection
    // so the call hears remote tracks from the first packet.
    let audio = match PlatformAudio::new() {
        Ok(a) => Some(a),
        Err(err) => {
            let _ = events.send(CallEvent::MicUnavailable(format!("{err}")));
            None
        }
    };
    let mut options = RoomOptions::default();
    options.auto_subscribe = true;
    options.adaptive_stream = true;
    options.dynacast = true;
    let (room, rx) = Room::connect(url, token, options)
        .await
        .map_err(|e| anyhow::anyhow!("LiveKit: {e}"))?;

    if !attempt.is_active() {
        let _ = room.close().await;
        anyhow::bail!("Call canceled");
    }

    // Everyone already in the room.
    for (_, p) in room.remote_participants() {
        let _ = events.send(CallEvent::Participant {
            identity: p.identity().to_string(),
            name: p.name(),
            metadata: p.metadata(),
            present: true,
        });
    }

    let mut session = Session {
        room: Arc::new(room),
        mic: None,
        _audio: None,
        camera: None,
        video_tasks: HashMap::new(),
        closed: false,
    };
    if let Some(audio) = audio {
        let track = LocalAudioTrack::create_audio_track("mic", audio.rtc_source());
        let publish = session
            .room
            .local_participant()
            .publish_track(
                LocalTrack::Audio(track.clone()),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    ..Default::default()
                },
            )
            .await;
        match publish {
            Ok(_) => session.mic = Some(track),
            Err(err) => {
                let _ = events.send(CallEvent::MicUnavailable(format!("publish: {err}")));
            }
        }
        session._audio = Some(audio);
    }
    if !attempt.is_active() {
        end_session(session).await;
        anyhow::bail!("Call canceled");
    }
    if camera {
        match publish_camera(&session.room, events).await {
            Ok(c) => session.camera = Some(c),
            Err(err) => {
                let _ = events.send(CallEvent::CameraUnavailable(format!("{err:#}")));
            }
        }
    }
    Ok((session, rx))
}

async fn end_session(mut s: Session) {
    for (_, task) in s.video_tasks.drain() {
        task.abort();
    }
    if let Some(c) = s.camera.take() {
        stop_camera(&s.room, c).await;
    }
    let _ = s.room.close().await;
    s.closed = true;
}

fn on_room_event(s: &mut Session, ev: RoomEvent, events: &CallEvents) {
    match ev {
        RoomEvent::ParticipantConnected(p) => {
            let _ = events.send(CallEvent::Participant {
                identity: p.identity().to_string(),
                name: p.name(),
                metadata: p.metadata(),
                present: true,
            });
        }
        RoomEvent::ParticipantDisconnected(p) => {
            let id = p.identity().to_string();
            if let Some(t) = s.video_tasks.remove(&id) {
                t.abort();
            }
            let _ = events.send(CallEvent::Participant {
                identity: id,
                name: p.name(),
                metadata: p.metadata(),
                present: false,
            });
        }
        RoomEvent::ParticipantMetadataChanged { participant, .. } => {
            let _ = events.send(CallEvent::Participant {
                identity: participant.identity().to_string(),
                name: participant.name(),
                metadata: participant.metadata(),
                present: true,
            });
        }
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Video(video),
            participant,
            ..
        } => {
            let identity = participant.identity().to_string();
            if let Some(t) = s.video_tasks.remove(&identity) {
                t.abort();
            }
            let stream = NativeVideoStream::new(video.rtc_track());
            let tx = events.clone();
            let id = identity.clone();
            s.video_tasks.insert(
                identity,
                tokio::spawn(async move { pump_video(stream, id, tx).await }),
            );
        }
        RoomEvent::TrackUnsubscribed {
            track, participant, ..
        } => {
            if matches!(track, RemoteTrack::Video(_)) {
                let identity = participant.identity().to_string();
                if let Some(t) = s.video_tasks.remove(&identity) {
                    t.abort();
                }
                let _ = events.send(CallEvent::VideoOff { identity });
            }
        }
        RoomEvent::TrackMuted {
            participant,
            publication,
        } => {
            if publication.kind() == TrackKind::Audio {
                let _ = events.send(CallEvent::RemoteMuted {
                    identity: participant.identity().to_string(),
                    muted: true,
                });
            } else {
                let _ = events.send(CallEvent::VideoOff {
                    identity: participant.identity().to_string(),
                });
            }
        }
        RoomEvent::TrackUnmuted {
            participant,
            publication,
        } => {
            if publication.kind() == TrackKind::Audio {
                let _ = events.send(CallEvent::RemoteMuted {
                    identity: participant.identity().to_string(),
                    muted: false,
                });
            }
        }
        RoomEvent::ParticipantAttributesChanged {
            changed_attributes, ..
        } => {
            if let Some(raw) = changed_attributes.get("agent_vision") {
                emit_agent_vision(events, raw);
            }
        }
        RoomEvent::DataReceived { payload, topic, .. } => {
            let text = String::from_utf8_lossy(&payload);
            if topic.as_deref() == Some("agent_vision") || text.contains("\"agent_vision\"") {
                emit_agent_vision(events, &text);
            }
        }
        RoomEvent::ActiveSpeakersChanged { speakers } => {
            let _ = events.send(CallEvent::Speaking(
                speakers.iter().map(|p| p.identity().to_string()).collect(),
            ));
        }
        RoomEvent::Disconnected { reason } => {
            let _ = events.send(CallEvent::Error(format!("Disconnected: {reason:?}")));
            let _ = events.send(CallEvent::Disconnected);
        }
        RoomEvent::ConnectionStateChanged(state) => {
            if state == ConnectionState::Reconnecting {
                let _ = events.send(CallEvent::Connecting);
            } else if state == ConnectionState::Connected {
                let _ = events.send(CallEvent::Connected);
            }
        }
        _ => {}
    }
}

/// Remote video → BGRA `RenderImage` tiles, throttled.
async fn pump_video(mut stream: NativeVideoStream, identity: String, events: CallEvents) {
    let mut last = Instant::now() - Duration::from_secs(1);
    while let Some(frame) = stream.next().await {
        if last.elapsed() < Duration::from_millis(TILE_FRAME_MS) {
            continue;
        }
        last = Instant::now();
        let buffer: &dyn VideoBuffer = frame.buffer.as_ref();
        let (w, h) = (buffer.width(), buffer.height());
        if w == 0 || h == 0 {
            continue;
        }
        let Some(mut src) = to_i420_any(buffer) else {
            continue;
        };
        // Scale down big frames before the texture upload.
        let (dw, dh) = if w > TILE_MAX_WIDTH {
            (
                TILE_MAX_WIDTH,
                (h as u64 * TILE_MAX_WIDTH as u64 / w as u64) as u32,
            )
        } else {
            (w, h)
        };
        let dw = dw & !1;
        let dh = dh & !1;
        let scaled = if (dw, dh) != (w, h) {
            src.scale(dw as i32, dh as i32)
        } else {
            src
        };
        let mut rgba = vec![0u8; (dw * dh * 4) as usize];
        scaled.to_argb(
            VideoFormatType::BGRA,
            &mut rgba,
            dw * 4,
            dw as i32,
            dh as i32,
        );
        let Some(img) = image::RgbaImage::from_raw(dw, dh, rgba) else {
            continue;
        };
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(img)]));
        if events
            .send(CallEvent::Video {
                identity: identity.clone(),
                frame: image,
            })
            .is_err()
        {
            break;
        }
    }
    let _ = events.send(CallEvent::VideoOff { identity });
}

/// Whatever libwebrtc decoded into (I420, NV12, a native buffer…) as I420.
fn to_i420_any(b: &dyn VideoBuffer) -> Option<I420Buffer> {
    if let Some(x) = b.as_i420() {
        return Some(x.to_i420());
    }
    if let Some(x) = b.as_native() {
        return Some(x.to_i420());
    }
    if let Some(x) = b.as_nv12() {
        return Some(x.to_i420());
    }
    if let Some(x) = b.as_i420a() {
        return Some(x.to_i420());
    }
    if let Some(x) = b.as_i422() {
        return Some(x.to_i420());
    }
    if let Some(x) = b.as_i444() {
        return Some(x.to_i420());
    }
    b.as_i010().map(|x| x.to_i420())
}

// ---------------------------------------------------------------------------
// Camera publish
// ---------------------------------------------------------------------------

async fn publish_camera(room: &Room, events: &CallEvents) -> anyhow::Result<CameraPublish> {
    let source = NativeVideoSource::new(
        VideoResolution {
            width: CAMERA_WIDTH,
            height: CAMERA_HEIGHT,
        },
        false,
    );
    let track =
        LocalVideoTrack::create_video_track("camera", RtcVideoSource::Native(source.clone()));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    camera::start(source, stop.clone(), events.clone())?;
    let camera = CameraPublish { track, stop };
    room.local_participant()
        .publish_track(
            LocalTrack::Video(camera.track.clone()),
            TrackPublishOptions {
                source: TrackSource::Camera,
                simulcast: false,
                video_codec: VideoCodec::VP8,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("publish camera: {e}"))?;
    Ok(camera)
}

async fn stop_camera(room: &Room, c: CameraPublish) {
    c.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = room
        .local_participant()
        .unpublish_track(&c.track.sid())
        .await;
}

/// §11: `{"type":"agent_vision","participant_identity":"u_…","seeing":bool,
/// "agent_name":…,"agent":"a_…","seeing_identities":[…]}` — the data message
/// (topic `agent_vision`) and the agent participant's `agent_vision`
/// attribute carry the same JSON → [`CallEvent::AgentVision`].
fn emit_agent_vision(events: &CallEvents, raw: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return;
    };
    let inner = v.get("agent_vision").unwrap_or(&v);
    if inner
        .get("type")
        .and_then(|t| t.as_str())
        .is_some_and(|t| t != "agent_vision")
    {
        return;
    }
    let identity = inner
        .get("participant_identity")
        .or_else(|| inner.get("identity"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let seeing = inner
        .get("seeing")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let seeing_identities: Vec<String> = inner
        .get("seeing_identities")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if identity.is_empty() && seeing_identities.is_empty() {
        return;
    }
    let _ = events.send(CallEvent::AgentVision {
        identity,
        seeing,
        seeing_identities,
    });
}

/// RGB → I420 (BT.601), the shape `NativeVideoSource` wants.
pub fn rgb_to_i420(width: u32, height: u32, rgb: &[u8]) -> I420Buffer {
    let mut buf = I420Buffer::new(width, height);
    let (sy, su, sv) = buf.strides();
    let (y, u, v) = buf.data_mut();
    for row in 0..height as usize {
        for col in 0..width as usize {
            let i = (row * width as usize + col) * 3;
            let (r, g, b) = (rgb[i] as f32, rgb[i + 1] as f32, rgb[i + 2] as f32);
            let yy = 16.0 + 0.257 * r + 0.504 * g + 0.098 * b;
            y[row * sy as usize + col] = yy.clamp(0.0, 255.0) as u8;
            if row % 2 == 0 && col % 2 == 0 {
                let uu = 128.0 - 0.148 * r - 0.291 * g + 0.439 * b;
                let vv = 128.0 + 0.439 * r - 0.368 * g - 0.071 * b;
                let ci = (row / 2) * su as usize + col / 2;
                u[ci] = uu.clamp(0.0, 255.0) as u8;
                v[(row / 2) * sv as usize + col / 2] = vv.clamp(0.0, 255.0) as u8;
            }
        }
    }
    buf
}

/// Camera capture. macOS: AVFoundation through `nokhwa` on its own thread,
/// ~15 fps, RGB frames converted to I420. Elsewhere there is no camera.
mod camera {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[cfg(target_os = "macos")]
    use std::sync::atomic::Ordering;

    use super::{CallEvent, CallEvents};
    #[cfg(target_os = "macos")]
    use super::{CAMERA_HEIGHT, CAMERA_WIDTH};
    use livekit::webrtc::prelude::*;
    use livekit::webrtc::video_source::native::NativeVideoSource;

    #[cfg(target_os = "macos")]
    pub fn start(
        source: NativeVideoSource,
        stop: Arc<AtomicBool>,
        events: CallEvents,
    ) -> anyhow::Result<()> {
        use nokhwa::pixel_format::RgbFormat;
        use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType, Resolution};
        use nokhwa::Camera;

        std::thread::Builder::new()
            .name("arbos-camera".into())
            .spawn(move || {
                let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
                    nokhwa::utils::CameraFormat::new(
                        Resolution::new(CAMERA_WIDTH, CAMERA_HEIGHT),
                        nokhwa::utils::FrameFormat::NV12,
                        15,
                    ),
                ));
                let opened = Camera::new(CameraIndex::Index(0), requested).and_then(|mut c| {
                    c.open_stream()?;
                    Ok(c)
                });
                let mut cam = match opened {
                    Ok(c) => c,
                    Err(err) => {
                        if std::env::var_os("ARBOS_SYNTHETIC_CAMERA").is_some() {
                            let _ = synthetic(source, stop, events);
                        } else {
                            let _ = events.send(CallEvent::CameraUnavailable(format!("{err}")));
                        }
                        return;
                    }
                };
                let mut last_tile = std::time::Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    let frame = match cam.frame() {
                        Ok(f) => f,
                        Err(_) => {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                            continue;
                        }
                    };
                    let Ok(img) = frame.decode_image::<RgbFormat>() else {
                        continue;
                    };
                    let (w, h) = img.dimensions();
                    let (w, h) = (w & !1, h & !1);
                    let rgb = img.as_raw();
                    let i420 = super::rgb_to_i420(w, h, &rgb[..(w * h * 3) as usize]);
                    let vf = VideoFrame {
                        rotation: VideoRotation::VideoRotation0,
                        timestamp_us: 0,
                        frame_metadata: None,
                        buffer: i420,
                    };
                    source.capture_frame(&vf);
                    // My own tile, mirrored like a selfie camera, ~10 fps.
                    if last_tile.elapsed() >= std::time::Duration::from_millis(100) {
                        last_tile = std::time::Instant::now();
                        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                        for row in 0..h as usize {
                            for col in (0..w as usize).rev() {
                                let i = (row * w as usize + col) * 3;
                                rgba.extend_from_slice(&[rgb[i + 2], rgb[i + 1], rgb[i], 255]);
                            }
                        }
                        if let Some(buf) = image::RgbaImage::from_raw(w, h, rgba) {
                            let _ = events.send(CallEvent::Video {
                                identity: "local".into(),
                                frame: Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                                    buf,
                                )])),
                            });
                        }
                    }
                }
                let _ = cam.stop_stream();
            })?;
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn start(
        source: NativeVideoSource,
        stop: Arc<AtomicBool>,
        events: CallEvents,
    ) -> anyhow::Result<()> {
        if std::env::var_os("ARBOS_SYNTHETIC_CAMERA").is_some() {
            return synthetic(source, stop, events);
        }
        anyhow::bail!("no camera on this platform yet")
    }

    /// `ARBOS_SYNTHETIC_CAMERA=1`: a moving test pattern instead of a camera
    /// (headless Macs, CI, demos) — proves the publish → receive path.
    pub fn synthetic(
        source: NativeVideoSource,
        stop: Arc<AtomicBool>,
        events: CallEvents,
    ) -> anyhow::Result<()> {
        use std::sync::atomic::Ordering;
        let (w, h) = (super::CAMERA_WIDTH, super::CAMERA_HEIGHT);
        std::thread::Builder::new()
            .name("arbos-synthetic-camera".into())
            .spawn(move || {
                let start = std::time::Instant::now();
                let mut rgb = vec![0u8; (w * h * 3) as usize];
                let mut n: u64 = 0;
                while !stop.load(Ordering::Relaxed) {
                    let t = start.elapsed().as_secs_f32();
                    for y in 0..h as usize {
                        for x in 0..w as usize {
                            let i = (y * w as usize + x) * 3;
                            let band = ((x as f32 / w as f32) * 7.0 + t * 0.5) as u32 % 7;
                            let (r, g, b) = match band {
                                0 => (235, 235, 235),
                                1 => (240, 200, 40),
                                2 => (40, 200, 220),
                                3 => (60, 200, 90),
                                4 => (220, 70, 200),
                                5 => (230, 60, 60),
                                _ => (50, 80, 230),
                            };
                            // A dark bar that sweeps down so motion is obvious.
                            let sweep = ((t * 60.0) as usize) % h as usize;
                            let dim = if y.abs_diff(sweep) < 12 { 0.35 } else { 1.0 };
                            rgb[i] = (r as f32 * dim) as u8;
                            rgb[i + 1] = (g as f32 * dim) as u8;
                            rgb[i + 2] = (b as f32 * dim) as u8;
                        }
                    }
                    let i420 = super::rgb_to_i420(w, h, &rgb);
                    source.capture_frame(&VideoFrame {
                        rotation: VideoRotation::VideoRotation0,
                        timestamp_us: 0,
                        frame_metadata: None,
                        buffer: i420,
                    });
                    if n % 3 == 0 {
                        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                        for px in rgb.chunks_exact(3) {
                            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                        }
                        if let Some(buf) = image::RgbaImage::from_raw(w, h, rgba) {
                            let _ = events.send(CallEvent::Video {
                                identity: "local".into(),
                                frame: Arc::new(gpui::RenderImage::new(vec![image::Frame::new(
                                    buf,
                                )])),
                            });
                        }
                    }
                    n += 1;
                    std::thread::sleep(std::time::Duration::from_millis(66));
                }
            })?;
        Ok(())
    }
}
