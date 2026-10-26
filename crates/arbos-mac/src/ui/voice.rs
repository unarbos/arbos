//! Voice (F11): microphone capture → speech-to-text through the relay proxy,
//! spoken replies (text-to-speech + playback), and the call strip that runs
//! the utterance loop (energy VAD → STT → send → reply → TTS). Audio never
//! streams to the box: each utterance is one WAV, one message.
//!
//! Without an input device (CI, Xvfb) the bundled test clip is fed in real
//! time and the UI says so; without an output device the reply is decoded
//! and timed but not heard.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use gpui::{
    div, prelude::*, px, App, Context, Entity, EventEmitter, Render, SharedString, Task, Window,
};

use crate::app::{AppEvent, AppState};
use crate::audio::{self, Pcm, RATE};
use crate::model::{now_ms, Id, Message, MessageStatus, Role};
use crate::sync::user_error::UserError;
use crate::ui::frost::{frosted, PANEL_BLUR};
use crate::ui::theme::Theme;

const TICK_MS: u64 = 50;
/// 20 ms frames for the VAD.
const FRAME: usize = (RATE / 50) as usize;
/// A press shorter than this is a toggle; longer is hold-to-talk.
const HOLD_MS: u64 = 350;
const MIN_MEMO_SECS: f32 = 0.3;
/// Memos longer than this are cut (STT input cap, keeps uploads small).
const MAX_MEMO_SECS: f32 = 120.0;
/// In test-clip mode a call feeds the clip this many times, then goes quiet
/// (a real relay would otherwise loop forever).
const TEST_CLIP_PLAYS: u8 = 2;

#[derive(Clone, Debug)]
pub enum VoiceEvent {
    /// A memo was transcribed for the composer: fill the input with this text.
    Transcript(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallPhase {
    Listening,
    Thinking,
    Speaking,
}

impl CallPhase {
    pub fn label(self) -> &'static str {
        match self {
            CallPhase::Listening => "Listening",
            CallPhase::Thinking => "Thinking",
            CallPhase::Speaking => "Speaking",
        }
    }
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

enum CaptureSource {
    Device {
        name: String,
        _stream: cpal::Stream,
    },
    /// No microphone: the bundled clip plays into the buffer at wall-clock speed.
    TestClip {
        clip: Arc<Pcm>,
        pos: usize,
        last_tick: u64,
        /// Clip restarts left (call mode); `0` = silence from now on.
        plays_left: u8,
        /// Do not feed before this time (pause between turns).
        resume_at: u64,
    },
}

struct Capture {
    source: CaptureSource,
    /// 16 kHz mono samples since the last drain.
    buf: Arc<Mutex<Vec<i16>>>,
    /// Latest chunk RMS as `f32` bits.
    level: Arc<AtomicU32>,
}

fn push_chunk(mono: Vec<i16>, rate: u32, buf: &Mutex<Vec<i16>>, level: &AtomicU32) {
    let resampled = audio::resample(&mono, rate, RATE);
    level.store(audio::rms(&resampled).to_bits(), Ordering::Relaxed);
    if let Ok(mut b) = buf.lock() {
        b.extend_from_slice(&resampled);
    }
}

fn open_device(
    buf: Arc<Mutex<Vec<i16>>>,
    level: Arc<AtomicU32>,
) -> Result<(String, cpal::Stream), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| "no input device".to_owned())?;
    let name = device.name().unwrap_or_else(|_| "microphone".to_owned());
    let config = device
        .default_input_config()
        .map_err(|e| format!("input config: {e}"))?;
    let rate = config.sample_rate().0;
    let channels = config.channels().max(1) as usize;
    let err_fn = |_err: cpal::StreamError| {};
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| {
                let mono = data
                    .chunks(channels)
                    .map(|f| {
                        let avg = f.iter().sum::<f32>() / f.len() as f32;
                        (avg.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
                    })
                    .collect();
                push_chunk(mono, rate, &buf, &level);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.into(),
            move |data: &[i16], _| {
                let mono = data
                    .chunks(channels)
                    .map(|f| (f.iter().map(|s| *s as i32).sum::<i32>() / f.len() as i32) as i16)
                    .collect();
                push_chunk(mono, rate, &buf, &level);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config.into(),
            move |data: &[u16], _| {
                let mono = data
                    .chunks(channels)
                    .map(|f| {
                        let avg = f.iter().map(|s| *s as i32).sum::<i32>() / f.len() as i32;
                        (avg - 32_768) as i16
                    })
                    .collect();
                push_chunk(mono, rate, &buf, &level);
            },
            err_fn,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| format!("input stream: {e}"))?;
    stream.play().map_err(|e| format!("input start: {e}"))?;
    Ok((name, stream))
}

impl Capture {
    /// The default microphone, or the test clip when there is none.
    fn open() -> (Self, Option<String>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let level = Arc::new(AtomicU32::new(0));
        match open_device(buf.clone(), level.clone()) {
            Ok((name, stream)) => (
                Self {
                    source: CaptureSource::Device {
                        name,
                        _stream: stream,
                    },
                    buf,
                    level,
                },
                None,
            ),
            Err(reason) => {
                eprintln!("voice: no input device ({reason}); using the bundled test clip");
                let clip = audio::decode_wav(audio::TEST_CLIP_WAV).unwrap_or_default();
                let clip = Arc::new(Pcm {
                    rate: RATE,
                    samples: audio::resample(&clip.samples, clip.rate.max(1), RATE),
                });
                (
                    Self {
                        source: CaptureSource::TestClip {
                            clip,
                            pos: 0,
                            last_tick: now_ms(),
                            plays_left: TEST_CLIP_PLAYS,
                            resume_at: 0,
                        },
                        buf,
                        level,
                    },
                    Some("No microphone — using the bundled test clip".to_string()),
                )
            }
        }
    }

    fn is_test_clip(&self) -> bool {
        matches!(self.source, CaptureSource::TestClip { .. })
    }

    /// True while the test clip still has samples to feed (false for a device).
    fn clip_active(&self) -> bool {
        match &self.source {
            CaptureSource::Device { .. } => false,
            CaptureSource::TestClip { clip, pos, .. } => *pos < clip.samples.len(),
        }
    }

    /// Call mode: play the clip once more after a short pause, if plays remain.
    fn restart_clip(&mut self) {
        if let CaptureSource::TestClip {
            clip,
            pos,
            plays_left,
            resume_at,
            ..
        } = &mut self.source
        {
            if *plays_left > 0 {
                *plays_left -= 1;
                *pos = 0;
                *resume_at = now_ms() + 800;
            } else {
                *pos = clip.samples.len();
            }
        }
    }

    /// Feed the test clip for the wall-clock time since the last tick.
    fn feed_clip(&mut self) {
        let now = now_ms();
        let CaptureSource::TestClip {
            clip,
            pos,
            last_tick,
            resume_at,
            ..
        } = &mut self.source
        else {
            return;
        };
        let elapsed = now.saturating_sub(*last_tick).min(500);
        *last_tick = now;
        if now < *resume_at {
            return;
        }
        let n = (elapsed * RATE as u64 / 1000) as usize;
        let end = (*pos + n).min(clip.samples.len());
        let chunk: Vec<i16> = if *pos < end {
            clip.samples[*pos..end].to_vec()
        } else {
            vec![0; n]
        };
        *pos = end.max(*pos);
        push_chunk(chunk, RATE, &self.buf, &self.level);
    }

    fn drain(&self) -> Vec<i16> {
        self.buf
            .lock()
            .map(|mut b| std::mem::take(&mut *b))
            .unwrap_or_default()
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Energy VAD
// ---------------------------------------------------------------------------

struct Vad {
    in_speech: bool,
    loud_frames: u32,
    silent_frames: u32,
    speech_frames: u32,
    /// Noise floor (EMA of quiet frames).
    noise: f32,
    pre_roll: Vec<i16>,
    utterance: Vec<i16>,
    carry: Vec<i16>,
}

impl Vad {
    const START_FRAMES: u32 = 3;
    /// 1.2 s of silence closes an utterance; a sentence pause (~0.8 s plus a
    /// quiet consonant tail) must not split a turn.
    const END_FRAMES: u32 = 60;
    /// 300 ms of speech is the minimum worth sending.
    const MIN_SPEECH_FRAMES: u32 = 15;
    const PRE_ROLL: usize = FRAME * 10;
    const MAX_UTTERANCE: usize = RATE as usize * 30;

    fn new() -> Self {
        Self {
            in_speech: false,
            loud_frames: 0,
            silent_frames: 0,
            speech_frames: 0,
            noise: 0.005,
            pre_roll: Vec::new(),
            utterance: Vec::new(),
            carry: Vec::new(),
        }
    }

    fn reset(&mut self) {
        let noise = self.noise;
        *self = Self::new();
        self.noise = noise;
    }

    fn threshold(&self) -> f32 {
        (self.noise * 3.0).clamp(0.012, 0.08)
    }

    /// Feed samples; returns a finished utterance when one just ended.
    fn feed(&mut self, samples: &[i16]) -> Option<Vec<i16>> {
        self.carry.extend_from_slice(samples);
        let mut done = None;
        while self.carry.len() >= FRAME {
            let frame: Vec<i16> = self.carry.drain(..FRAME).collect();
            let level = audio::rms(&frame);
            let loud = level > self.threshold();
            if !self.in_speech {
                if !loud {
                    self.noise = self.noise * 0.95 + level * 0.05;
                }
                self.pre_roll.extend_from_slice(&frame);
                if self.pre_roll.len() > Self::PRE_ROLL {
                    let cut = self.pre_roll.len() - Self::PRE_ROLL;
                    self.pre_roll.drain(..cut);
                }
                if loud {
                    self.loud_frames += 1;
                    if self.loud_frames >= Self::START_FRAMES {
                        self.in_speech = true;
                        self.utterance = std::mem::take(&mut self.pre_roll);
                        self.speech_frames = self.loud_frames;
                        self.silent_frames = 0;
                    }
                } else {
                    self.loud_frames = 0;
                }
                continue;
            }
            self.utterance.extend_from_slice(&frame);
            if loud {
                self.speech_frames += 1;
                self.silent_frames = 0;
            } else {
                self.silent_frames += 1;
            }
            let too_long = self.utterance.len() >= Self::MAX_UTTERANCE;
            if self.silent_frames >= Self::END_FRAMES || too_long {
                let keep = self.speech_frames >= Self::MIN_SPEECH_FRAMES;
                let utterance = std::mem::take(&mut self.utterance);
                self.in_speech = false;
                self.loud_frames = 0;
                self.silent_frames = 0;
                self.speech_frames = 0;
                if keep && done.is_none() {
                    done = Some(utterance);
                }
            }
        }
        done
    }
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

enum Player {
    Device {
        _stream: rodio::OutputStream,
        handle: rodio::OutputStreamHandle,
        sink: Option<rodio::Sink>,
    },
    /// No speaker: playback is timed, not heard.
    Silent,
}

impl Player {
    fn open() -> (Self, Option<String>) {
        match rodio::OutputStream::try_default() {
            Ok((stream, handle)) => (
                Self::Device {
                    _stream: stream,
                    handle,
                    sink: None,
                },
                None,
            ),
            Err(e) => (Self::Silent, Some(format!("No speaker ({e})"))),
        }
    }

    /// Start playing a WAV; returns an error string when it cannot be decoded.
    fn play(&mut self, bytes: &[u8]) -> Result<(), String> {
        match self {
            Player::Device { handle, sink, .. } => {
                let decoder = rodio::Decoder::new(Cursor::new(bytes.to_vec()))
                    .map_err(|e| format!("decode: {e}"))?;
                let s = rodio::Sink::try_new(handle).map_err(|e| format!("sink: {e}"))?;
                s.append(decoder);
                s.play();
                *sink = Some(s);
                Ok(())
            }
            Player::Silent => Ok(()),
        }
    }

    fn stop(&mut self) {
        if let Player::Device { sink, .. } = self {
            if let Some(s) = sink.take() {
                s.stop();
            }
        }
    }

    fn is_playing(&self) -> bool {
        match self {
            Player::Device { sink, .. } => sink.as_ref().is_some_and(|s| !s.empty()),
            Player::Silent => false,
        }
    }
}

// ---------------------------------------------------------------------------
// The entity
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SttTarget {
    /// Fill the composer; the user confirms with Enter.
    Composer,
    /// Voice mode: send at once with the memo attached.
    Send,
    /// Call mode: send, then wait for the reply and speak it.
    Call,
}

struct PendingStt {
    request_id: u64,
    target: SttTarget,
    wav: Arc<Vec<u8>>,
    started_at: u64,
}

struct Recording {
    started_at: u64,
    samples: Vec<i16>,
    /// Keep recording after the test clip ends (screenshot rig) instead of
    /// stopping when it runs out.
    hold: bool,
}

struct Call {
    started_at: u64,
    phase: CallPhase,
    muted: bool,
    vad: Vad,
    /// Voice mode was off before the call: switch it back off on hang-up.
    voice_was_off: bool,
}

pub struct Voice {
    state: Entity<AppState>,
    capture: Option<Capture>,
    recording: Option<Recording>,
    ignore_release: bool,
    pending_stt: Option<PendingStt>,
    call: Option<Call>,
    player: Option<Player>,
    /// Cached speech per assistant message (speaker icon replays it).
    speech: HashMap<Id, Arc<Vec<u8>>>,
    /// TTS request id → (message, play when it arrives).
    pending_tts: HashMap<u64, (Id, bool)>,
    /// Replies already spoken or skipped.
    seen: HashSet<Id>,
    /// Only replies created after this are spoken (voice mode on / call start).
    voice_since: u64,
    next_request: u64,
    now_speaking: Option<Id>,
    /// Timed end of the current playback (also drives the silent player).
    speaking_until: Option<u64>,
    /// One line about the current state ("No microphone — …", "Spoke 4.2 s").
    notice: Option<Notice>,
    /// The last STT/TTS answer came from a mock.
    mock: bool,
    tick: Option<Task<()>>,
    level: f32,
}

impl EventEmitter<VoiceEvent> for Voice {}

/// The composer's one-line voice note. A note about a reply (synthesising,
/// speaking, could not be decoded) belongs to the chat that reply is in: it is
/// shown only there and dropped when the user moves to another chat, so a
/// failure in one chat never becomes a banner in every chat. Notes about the
/// device path (no microphone, call ended) have no chat and show anywhere.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    text: String,
    chat: Option<Id>,
}

impl Notice {
    fn global(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            chat: None,
        }
    }

    fn in_chat(text: impl Into<String>, chat: Option<Id>) -> Self {
        Self {
            text: text.into(),
            chat,
        }
    }

    /// Visible in `active` chat?
    fn shows_in(&self, active: Option<&str>) -> bool {
        match &self.chat {
            None => true,
            Some(chat) => active == Some(chat.as_str()),
        }
    }
}

impl Voice {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |this, _, cx| this.check_replies(cx))
            .detach();
        cx.subscribe(&state, |this, _, event: &AppEvent, cx| match event {
            AppEvent::VoiceTranscript {
                request_id,
                text,
                error,
                mock,
            } => this.on_transcript(*request_id, text, error.as_deref(), *mock, cx),
            AppEvent::VoiceSpeech {
                request_id,
                bytes,
                mime,
                error,
                mock,
            } => this.on_speech(
                *request_id,
                bytes.clone(),
                mime,
                error.as_deref(),
                *mock,
                cx,
            ),
            AppEvent::ReplaySpeech { message_id } => this.replay(message_id.clone(), cx),
            AppEvent::TranscriptText { .. } | AppEvent::TranscriptChanged { .. } => {}
        })
        .detach();
        let mut this = Self {
            state,
            capture: None,
            recording: None,
            ignore_release: false,
            pending_stt: None,
            call: None,
            player: None,
            speech: HashMap::new(),
            pending_tts: HashMap::new(),
            seen: HashSet::new(),
            voice_since: u64::MAX,
            next_request: 0,
            now_speaking: None,
            speaking_until: None,
            notice: None,
            mock: false,
            tick: None,
            level: 0.0,
        };
        // Rig: `ARBOS_SHOW=recording` opens the composer mid-memo and keeps it
        // there (the test clip feeds silence once it ends) until Stop / Esc.
        if std::env::var("ARBOS_SHOW").is_ok_and(|v| v == "recording") {
            this.start_recording(true, cx);
        }
        this
    }

    // ----- read-only state for the composer and the strip --------------------

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn recording_ms(&self) -> u64 {
        self.recording
            .as_ref()
            .map(|r| now_ms().saturating_sub(r.started_at))
            .unwrap_or(0)
    }

    pub fn is_transcribing(&self) -> bool {
        self.pending_stt.is_some()
    }

    pub fn in_call(&self) -> bool {
        self.call.is_some()
    }

    /// 0.0–1.0 input level, scaled for a meter (speech peaks near 1).
    pub fn level(&self) -> f32 {
        (self.level * 6.0).min(1.0)
    }

    /// The note the composer shows in `active_chat` (chat-scoped notes stay
    /// in their own chat).
    pub fn notice_for(&self, active_chat: Option<&str>) -> Option<&str> {
        self.notice
            .as_ref()
            .filter(|n| n.shows_in(active_chat))
            .map(|n| n.text.as_str())
    }

    fn set_notice(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice::global(text));
    }

    /// A note about `message_id`'s speech, scoped to the chat that holds it.
    fn set_reply_notice(&mut self, text: impl Into<String>, message_id: &str, cx: &App) {
        let state = self.state.read(cx);
        let chat = state
            .chat_of_message(message_id)
            .or_else(|| state.active_chat_id());
        self.notice = Some(Notice::in_chat(text, chat));
    }

    /// A chat-scoped note does not follow the user to another chat.
    fn drop_stale_notice(&mut self, cx: &App) {
        let active = self.state.read(cx).active_chat_id();
        if self
            .notice
            .as_ref()
            .is_some_and(|n| n.chat.is_some() && !n.shows_in(active.as_deref()))
        {
            self.notice = None;
        }
    }

    pub fn using_test_clip(&self) -> bool {
        self.capture.as_ref().is_some_and(Capture::is_test_clip)
    }

    fn voice_mode(&self, cx: &App) -> bool {
        self.state.read(cx).voice_mode
    }

    // ----- voice mode ---------------------------------------------------------

    fn set_voice_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        // Relay kill switch (features.voice_mode): the switch cannot be turned on while off.
        if on && !self.state.read(cx).voice_mode_available() {
            return;
        }
        if on {
            self.voice_since = now_ms();
        } else {
            self.voice_since = u64::MAX;
            if self.call.is_none() {
                self.stop_playback();
            }
        }
        self.state.update(cx, |s, cx| s.set_voice_mode(on, cx));
        cx.notify();
    }

    // ----- memo button --------------------------------------------------------

    /// Mouse down on the mic: start recording, or stop a toggled recording.
    pub fn mic_pressed(&mut self, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            self.ignore_release = true;
            self.stop_recording(cx);
            return;
        }
        if self.pending_stt.is_some() {
            return;
        }
        self.ignore_release = false;
        self.start_recording(false, cx);
    }

    /// Mouse up on the mic: a long press was hold-to-talk and stops here.
    pub fn mic_released(&mut self, cx: &mut Context<Self>) {
        if self.ignore_release {
            self.ignore_release = false;
            return;
        }
        if self.recording_ms() >= HOLD_MS && self.recording.is_some() {
            self.stop_recording(cx);
        }
    }

    /// The Stop button / Esc while a memo records: stop the microphone and
    /// transcribe what was said so far (the text lands in the composer).
    pub fn stop_memo(&mut self, cx: &mut Context<Self>) {
        self.ignore_release = false;
        self.stop_recording(cx);
    }

    /// The ✕ beside the recording row: throw the memo away. While the relay is
    /// still transcribing, forget that request instead (nothing is inserted).
    pub fn cancel_memo(&mut self, cx: &mut Context<Self>) {
        self.ignore_release = false;
        if self.recording.take().is_some() {
            if self.call.is_none() {
                self.capture = None;
            }
            self.set_notice("Recording discarded");
        } else if self.pending_stt.take().is_some() {
            self.set_notice("Transcription cancelled");
        } else {
            return;
        }
        cx.notify();
    }

    /// Esc: stop a running memo (keeping its transcript) or cancel a
    /// transcription in flight. Returns false when there was nothing to do.
    pub fn escape(&mut self, cx: &mut Context<Self>) -> bool {
        match escape_action(self.recording.is_some(), self.pending_stt.is_some()) {
            Some(EscapeAction::StopMemo) => self.stop_memo(cx),
            Some(EscapeAction::CancelTranscription) => self.cancel_memo(cx),
            None => return false,
        }
        true
    }

    fn ensure_capture(&mut self) {
        if self.capture.is_none() {
            let (capture, notice) = Capture::open();
            self.notice = notice
                .or_else(|| match &capture.source {
                    CaptureSource::Device { name, .. } => Some(format!("Microphone: {name}")),
                    CaptureSource::TestClip { .. } => None,
                })
                .map(Notice::global);
            self.capture = Some(capture);
        }
    }

    fn start_recording(&mut self, hold: bool, cx: &mut Context<Self>) {
        self.ensure_capture();
        if let Some(c) = &self.capture {
            c.drain();
        }
        self.recording = Some(Recording {
            started_at: now_ms(),
            samples: Vec::new(),
            hold,
        });
        self.ensure_tick(cx);
        cx.notify();
    }

    fn stop_recording(&mut self, cx: &mut Context<Self>) {
        let Some(mut rec) = self.recording.take() else {
            return;
        };
        if let Some(c) = &self.capture {
            rec.samples.extend(c.drain());
        }
        if self.call.is_none() {
            self.capture = None;
        }
        let secs = rec.samples.len() as f32 / RATE as f32;
        if secs < MIN_MEMO_SECS {
            self.set_notice("Too short — hold the mic a little longer");
            cx.notify();
            return;
        }
        let target = if self.voice_mode(cx) {
            SttTarget::Send
        } else {
            SttTarget::Composer
        };
        self.request_stt(rec.samples, target, cx);
    }

    fn request_stt(&mut self, samples: Vec<i16>, target: SttTarget, cx: &mut Context<Self>) {
        let max = (MAX_MEMO_SECS * RATE as f32) as usize;
        let samples = if samples.len() > max {
            &samples[..max]
        } else {
            &samples[..]
        };
        let wav = Arc::new(audio::encode_wav(RATE, samples));
        self.next_request += 1;
        let request_id = self.next_request;
        self.pending_stt = Some(PendingStt {
            request_id,
            target,
            wav: wav.clone(),
            started_at: now_ms(),
        });
        self.set_notice(format!(
            "Transcribing {:.1} s…",
            samples.len() as f32 / RATE as f32
        ));
        self.state.update(cx, |s, _| s.transcribe(request_id, wav));
        cx.notify();
    }

    fn on_transcript(
        &mut self,
        request_id: u64,
        text: &str,
        error: Option<&str>,
        mock: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_stt.take_if(|p| p.request_id == request_id) else {
            return;
        };
        self.mock = mock;
        let took = now_ms().saturating_sub(pending.started_at);
        let text = text.trim();
        if let Some(err) = error {
            // A failure is the composer's note (the shared error element), not a muted line.
            let error = UserError::from_text("transcribe your memo", err);
            self.notice = None;
            self.state.update(cx, |s, cx| {
                if let Some(chat) = s.active_chat_id() {
                    s.set_send_error(chat, error);
                }
                cx.notify();
            });
            if let Some(call) = &mut self.call {
                call.phase = CallPhase::Listening;
            }
            cx.notify();
            return;
        }
        if text.is_empty() {
            self.set_notice("Heard nothing");
            if let Some(call) = &mut self.call {
                call.phase = CallPhase::Listening;
            }
            cx.notify();
            return;
        }
        self.set_notice(format!(
            "Transcribed in {:.1} s{}",
            took as f32 / 1000.0,
            if mock { " (mock)" } else { "" }
        ));
        match pending.target {
            SttTarget::Composer => cx.emit(VoiceEvent::Transcript(text.to_owned())),
            SttTarget::Send | SttTarget::Call => {
                let text = text.to_owned();
                let wav = pending.wav.clone();
                self.state.update(cx, |s, cx| s.send_voice(text, wav, cx));
                if let Some(call) = &mut self.call {
                    call.phase = CallPhase::Thinking;
                }
            }
        }
        cx.notify();
    }

    // ----- spoken replies -----------------------------------------------------

    /// Called on every state change: a reply that just finished in the active
    /// chat is spoken while voice mode is on (or during a call).
    fn check_replies(&mut self, cx: &mut Context<Self>) {
        self.drop_stale_notice(cx);
        // Feature off: never request speech, whatever an older state said.
        if self.voice_since == u64::MAX || !self.state.read(cx).voice_mode_available() {
            cx.notify();
            return;
        }
        let candidate: Option<Message> = {
            let state = self.state.read(cx);
            state.active_chat_id().and_then(|chat_id| {
                state
                    .messages_of(&chat_id)
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::Assistant)
                    .cloned()
            })
        };
        let Some(reply) = candidate else {
            cx.notify();
            return;
        };
        if reply.created_at < self.voice_since || self.seen.contains(&reply.id) {
            cx.notify();
            return;
        }
        match reply.status {
            MessageStatus::Queued | MessageStatus::Streaming => {}
            MessageStatus::Done => {
                self.seen.insert(reply.id.clone());
                let text = reply.plain_text();
                if text.trim().is_empty() {
                    if let Some(call) = &mut self.call {
                        call.phase = CallPhase::Listening;
                    }
                } else {
                    self.request_tts(reply.id.clone(), text, true, cx);
                }
            }
            MessageStatus::Error | MessageStatus::Cancelled => {
                self.seen.insert(reply.id.clone());
                if let Some(call) = &mut self.call {
                    call.phase = CallPhase::Listening;
                }
            }
        }
        cx.notify();
    }

    fn request_tts(
        &mut self,
        message_id: Id,
        text: String,
        autoplay: bool,
        cx: &mut Context<Self>,
    ) {
        self.next_request += 1;
        let request_id = self.next_request;
        self.pending_tts
            .insert(request_id, (message_id.clone(), autoplay));
        if let Some(call) = &mut self.call {
            call.phase = CallPhase::Thinking;
        }
        self.set_reply_notice("Synthesising the reply…", &message_id, cx);
        self.state.update(cx, |s, _| s.speak(request_id, text));
    }

    fn on_speech(
        &mut self,
        request_id: u64,
        bytes: Arc<Vec<u8>>,
        mime: &str,
        error: Option<&str>,
        mock: bool,
        cx: &mut Context<Self>,
    ) {
        let Some((message_id, autoplay)) = self.pending_tts.remove(&request_id) else {
            return;
        };
        self.mock = mock;
        if let Some(err) = error {
            self.fail_speech(
                &message_id,
                UserError::from_text("play the reply", err).sentence(),
                cx,
            );
            return;
        }
        if !mime.starts_with("audio/") && audio::decode_wav(&bytes).is_none() {
            self.fail_speech(
                &message_id,
                format!("Speech came back as {mime}, not audio"),
                cx,
            );
            return;
        }
        self.speech.insert(message_id.clone(), bytes.clone());
        self.state
            .update(cx, |s, cx| s.mark_spoken(message_id.clone(), cx));
        if autoplay {
            self.play(message_id, bytes, cx);
        }
        cx.notify();
    }

    /// Speech for one reply did not arrive or did not play: the note stays in
    /// that reply's chat and the reply's speaker row says so; nothing global.
    fn fail_speech(&mut self, message_id: &Id, reason: String, cx: &mut Context<Self>) {
        self.set_reply_notice(reason.clone(), message_id, cx);
        self.state.update(cx, |s, cx| {
            s.mark_speech_failed(message_id.clone(), reason, cx)
        });
        if let Some(call) = &mut self.call {
            call.phase = CallPhase::Listening;
        }
        cx.notify();
    }

    fn replay(&mut self, message_id: Id, cx: &mut Context<Self>) {
        if !self.state.read(cx).voice_mode_available() {
            return;
        }
        if let Some(bytes) = self.speech.get(&message_id).cloned() {
            self.play(message_id, bytes, cx);
            return;
        }
        let text = {
            let state = self.state.read(cx);
            state.active_chat_id().and_then(|chat_id| {
                state
                    .messages_of(&chat_id)
                    .iter()
                    .find(|m| m.id == message_id)
                    .map(Message::plain_text)
            })
        };
        if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
            self.seen.insert(message_id.clone());
            self.request_tts(message_id, text, true, cx);
            cx.notify();
        }
    }

    fn play(&mut self, message_id: Id, bytes: Arc<Vec<u8>>, cx: &mut Context<Self>) {
        self.stop_playback();
        let Some(pcm) = audio::decode_wav(&bytes) else {
            self.fail_speech(
                &message_id,
                UserError::from_text("play the reply", "Reply audio could not be decoded")
                    .sentence(),
                cx,
            );
            return;
        };
        let secs = pcm.duration_secs();
        if self.player.is_none() {
            let (player, notice) = Player::open();
            self.player = Some(player);
            if let Some(n) = notice {
                self.set_notice(n);
            }
        }
        let silent = matches!(self.player, Some(Player::Silent) | None);
        if let Some(player) = &mut self.player {
            if let Err(err) = player.play(&bytes) {
                self.fail_speech(
                    &message_id,
                    UserError::from_text("play the reply", err.to_string()).sentence(),
                    cx,
                );
                return;
            }
        }
        // This reply plays: a failure note from an earlier attempt is over.
        self.state
            .update(cx, |s, cx| s.clear_speech_failed(&message_id, cx));
        self.now_speaking = Some(message_id.clone());
        self.speaking_until = Some(now_ms() + (secs * 1000.0) as u64);
        if let Some(call) = &mut self.call {
            call.phase = CallPhase::Speaking;
            // Nothing the mic heard during our own speech counts.
            call.vad.reset();
        }
        let text = if silent {
            format!(
                "No speaker — reply decoded ({secs:.1} s, {} kHz){}",
                pcm.rate / 1000,
                if self.mock { ", mock" } else { "" }
            )
        } else {
            format!(
                "Speaking {secs:.1} s{}",
                if self.mock { " (mock)" } else { "" }
            )
        };
        self.set_reply_notice(text, &message_id, cx);
        self.ensure_tick(cx);
        cx.notify();
    }

    fn stop_playback(&mut self) {
        if let Some(p) = &mut self.player {
            p.stop();
        }
        self.speaking_until = None;
        self.now_speaking = None;
    }

    fn on_speech_end(&mut self) {
        self.speaking_until = None;
        self.now_speaking = None;
        if let Some(call) = &mut self.call {
            call.phase = CallPhase::Listening;
            call.vad.reset();
            if let Some(c) = &mut self.capture {
                c.drain();
                c.restart_clip();
            }
        }
    }

    // ----- call mode ----------------------------------------------------------

    pub fn toggle_call(&mut self, cx: &mut Context<Self>) {
        if self.call.is_some() {
            self.hang_up(cx);
        } else {
            self.start_call(cx);
        }
    }

    fn start_call(&mut self, cx: &mut Context<Self>) {
        let voice_was_off = !self.voice_mode(cx);
        if voice_was_off {
            self.set_voice_mode(true, cx);
        } else {
            self.voice_since = now_ms();
        }
        self.recording = None;
        self.ensure_capture();
        if let Some(c) = &self.capture {
            c.drain();
        }
        self.call = Some(Call {
            started_at: now_ms(),
            phase: CallPhase::Listening,
            muted: false,
            vad: Vad::new(),
            voice_was_off,
        });
        self.ensure_tick(cx);
        cx.notify();
    }

    pub fn hang_up(&mut self, cx: &mut Context<Self>) {
        let Some(call) = self.call.take() else {
            return;
        };
        self.stop_playback();
        self.capture = None;
        self.pending_stt = None;
        if call.voice_was_off {
            self.set_voice_mode(false, cx);
        }
        self.set_notice(format!(
            "Call ended after {}",
            mmss(now_ms().saturating_sub(call.started_at))
        ));
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(call) = &mut self.call {
            call.muted = !call.muted;
            call.vad.reset();
            if let Some(c) = &self.capture {
                c.drain();
            }
        }
        cx.notify();
    }

    // ----- tick ---------------------------------------------------------------

    fn ensure_tick(&mut self, cx: &mut Context<Self>) {
        if self.tick.is_some() {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(TICK_MS))
                .await;
            match this.update(cx, |v, cx| v.tick(cx)) {
                Ok(true) => {}
                Ok(false) | Err(_) => break,
            }
        }));
    }

    /// Returns false when nothing is active any more (the loop ends).
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(c) = &mut self.capture {
            c.feed_clip();
        }
        let samples = self
            .capture
            .as_ref()
            .map(Capture::drain)
            .unwrap_or_default();
        self.level = self.capture.as_ref().map(Capture::level).unwrap_or(0.0);

        if let Some(rec) = &mut self.recording {
            rec.samples.extend_from_slice(&samples);
            let clip_done = !rec.hold
                && self
                    .capture
                    .as_ref()
                    .is_some_and(|c| c.is_test_clip() && !c.clip_active());
            let too_long = rec.samples.len() as f32 / RATE as f32 >= MAX_MEMO_SECS;
            if clip_done || too_long {
                self.stop_recording(cx);
            }
        } else if let Some(call) = &mut self.call {
            let listening = call.phase == CallPhase::Listening && !call.muted;
            if call.muted {
                self.level = 0.0;
            }
            let utterance = if listening && self.pending_stt.is_none() {
                call.vad.feed(&samples)
            } else {
                None
            };
            if let Some(utterance) = utterance {
                self.request_stt(utterance, SttTarget::Call, cx);
            }
        }

        if self.now_speaking.is_some() {
            let timed_out = self.speaking_until.is_some_and(|t| now_ms() >= t);
            let device_done = match &self.player {
                Some(Player::Device { .. }) => !self.player.as_ref().unwrap().is_playing(),
                Some(Player::Silent) | None => true,
            };
            if timed_out && device_done {
                self.on_speech_end();
            }
        }

        cx.notify();
        let active = self.capture.is_some() || self.now_speaking.is_some() || self.call.is_some();
        if !active {
            self.tick = None;
        }
        active
    }

    // ----- call strip ---------------------------------------------------------

    fn render_meter(&self, theme: &Theme, active: bool) -> impl IntoElement {
        let level = if active { self.level() } else { 0.0 };
        let mut bars = div().flex().items_end().gap(px(2.0)).h(px(18.0));
        for i in 0..12 {
            let lit = level > i as f32 / 12.0;
            let h = 4.0 + 14.0 * ((i as f32 + 1.0) / 12.0);
            bars = bars.child(div().w(px(3.0)).h(px(h)).rounded(px(1.5)).bg(if lit {
                theme.accent
            } else {
                theme.border_strong
            }));
        }
        bars
    }
}

fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EscapeAction {
    /// Stop the microphone; what was said so far is still transcribed.
    StopMemo,
    /// Drop the STT request in flight; nothing is inserted.
    CancelTranscription,
}

/// What Esc does to the memo path: a live recording stops (and keeps its
/// text), a transcription in flight is cancelled, otherwise Esc is not ours.
fn escape_action(recording: bool, transcribing: bool) -> Option<EscapeAction> {
    if recording {
        Some(EscapeAction::StopMemo)
    } else if transcribing {
        Some(EscapeAction::CancelTranscription)
    } else {
        None
    }
}

/// Thin recording/level row for the composer while a memo records (the voice
/// tick re-renders it every 50 ms).
pub fn render_recording_row(voice: &Voice, theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(div().size(px(9.0)).rounded_full().bg(theme.danger))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme.text)
                .child(format!("Listening… {}", mmss(voice.recording_ms()))),
        )
        .child(voice.render_meter(theme, true))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme.text_faint)
                .child(if voice.using_test_clip() {
                    "no microphone: recording the bundled test clip"
                } else {
                    "Stop or Esc to transcribe · ✕ discards"
                }),
        )
}

impl Render for Voice {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let Some(call) = &self.call else {
            return div().into_any_element();
        };
        let phase = call.phase;
        let muted = call.muted;
        let elapsed = mmss(now_ms().saturating_sub(call.started_at));
        let (dot, label) = match phase {
            CallPhase::Listening => (
                theme.accent,
                if muted {
                    "Muted".to_owned()
                } else {
                    CallPhase::Listening.label().to_owned()
                },
            ),
            CallPhase::Thinking => (theme.warn, format!("{}…", CallPhase::Thinking.label())),
            CallPhase::Speaking => (theme.text, CallPhase::Speaking.label().to_owned()),
        };
        let mut sub: Vec<String> = Vec::new();
        if self.using_test_clip() {
            sub.push("test clip as microphone".into());
        }
        if matches!(self.player, Some(Player::Silent)) {
            sub.push("no speaker".into());
        }
        if self.mock {
            sub.push("mock STT/TTS".into());
        }
        // The call strip sits in the active chat: same scoping as the composer line.
        let active_chat = self.state.read(cx).active_chat_id();
        if let Some(n) = self.notice_for(active_chat.as_deref()) {
            sub.push(n.to_owned());
        }
        let subline = sub.join(" · ");

        let button = |id: &'static str, glyph: &'static str, label: &'static str, danger: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(if danger {
                    theme.danger
                } else {
                    theme.border_strong
                })
                .text_size(px(12.0))
                .text_color(if danger { theme.danger } else { theme.text })
                .hover(|s| s.bg(theme.surface_2))
                .cursor_pointer()
                .child(div().text_size(px(13.0)).child(glyph))
                .child(label)
        };

        let strip = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(14.0))
            .py(px(10.0))
            .rounded(px(14.0))
            .bg(theme.card())
            .border_1()
            .border_color(theme.border_strong)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .w(px(120.0))
                            .child(div().size(px(9.0)).rounded_full().bg(dot))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .child(label),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(theme.text_muted)
                            .w(px(44.0))
                            .child(elapsed),
                    )
                    .child(self.render_meter(&theme, phase == CallPhase::Listening && !muted))
                    .child(div().flex_1())
                    .child(
                        button(
                            "call-mute",
                            if muted { "⊘" } else { "◉" },
                            if muted { "Unmute" } else { "Mute" },
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))),
                    )
                    .child(
                        button("call-hangup", "✕", "Hang up", true)
                            .on_click(cx.listener(|this, _, _, cx| this.hang_up(cx))),
                    ),
            )
            .when(!subline.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.text_faint)
                        .child(SharedString::from(subline)),
                )
            });
        div()
            .w_full()
            .flex()
            .justify_center()
            .px(px(28.0))
            .pt(px(8.0))
            .child(
                div()
                    .w_full()
                    .max_w(px(crate::ui::theme::COMPOSER_WIDTH
                        .max(crate::ui::transcript::COLUMN_MAX_WIDTH - 56.0)))
                    .child(frosted(14.0, PANEL_BLUR, strip)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_stops_a_recording_before_anything_else() {
        assert_eq!(escape_action(true, false), Some(EscapeAction::StopMemo));
        assert_eq!(escape_action(true, true), Some(EscapeAction::StopMemo));
        assert_eq!(
            escape_action(false, true),
            Some(EscapeAction::CancelTranscription)
        );
        assert_eq!(escape_action(false, false), None);
    }
}
