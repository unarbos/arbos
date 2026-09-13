//! Voice over the self-hosted speech server.
//!
//! One WebSocket, the protocol of `ios/Arbos/Voice/SelfHostedVoiceSession.swift`:
//! binary frames are PCM16 mono 24 kHz both ways; JSON text frames carry
//! control. The server does speech only — the reply text comes from the
//! kernel and is handed back here with [`speak`].
//!
//! On macOS the microphone is CoreAudio in this process (see [`Mic`]), so
//! the Privacy › Microphone prompt and row are the app's own. Elsewhere the
//! microphone, and everywhere the speaker, are external processes
//! (PipeWire, Pulse, ALSA, sox, ffmpeg — first one found).
//! `ARBOS_VOICE_MIC_CMD` / `ARBOS_VOICE_PLAYER_CMD` replace them (tests
//! feed a file and swallow the output).
//!
//! The composer keeps its push-to-talk shape: [`start`] opens the mic,
//! [`peek`] gives the words so far, [`stop`] returns the take. The server's
//! own end-of-speech also closes a take.

use anyhow::{Result, anyhow, bail};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes, client::IntoClientRequest};

pub const RATE: u32 = 24_000;
/// 100 ms of PCM16 mono at 24 kHz.
const CHUNK: usize = (RATE as usize / 10) * 2;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// After a reply ends, how long the mic still counts as hearing the speaker.
const ECHO_TAIL: Duration = Duration::from_millis(400);
/// After Stop, how long the final transcript may take to arrive.
const FINAL_WAIT: Duration = Duration::from_millis(2_000);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceCfg {
    pub url: String,
    pub token: Option<String>,
    /// Show what the server's own agent does (`agent.*`, `tool.*` frames)
    /// as notices in the chat. `voice_mirror = false` turns it off.
    pub mirror: bool,
    /// Who answers on the server side: `none` (this window's kernel
    /// answers dictation; `/voice` has nobody to talk to), `kernel` (the
    /// server's own kernel agent), or `openrouter` (its model with the
    /// Arbos tools). `voice_reply` in config.toml; default `none`.
    pub reply: String,
}

/// One thing the speech server's agent did, for the chat to show.
#[derive(Debug, Clone)]
pub struct Mirror {
    /// `agent.event`, `agent.done`, `agent.turn`, `tool.call`,
    /// `tool.result`, `text.done`, …
    pub kind: String,
    pub agent: String,
    pub text: String,
}

/// What the client is doing, for the composer's status row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Off,
    Connecting,
    Ready,
    Listening,
    Speaking,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Listening => "listening",
            Self::Speaking => "speaking",
        }
    }
}

/// What the session is for. A dictation session opens the mic per take and
/// leaves replies to this window's kernel. A call keeps the mic open, talks
/// to the project's main agent through the gateway's narrator, and plays
/// every reply the gateway starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionKind {
    Dictation,
    Call {
        /// `<machine>/<project>` for the gateway to pick the kernel; the
        /// tab's label today.
        project: String,
    },
}

/// A snapshot for the UI: the take so far and the state.
#[derive(Debug, Clone, Default)]
pub struct Peek {
    /// Final segments of the take plus the live partial.
    pub text: String,
    pub phase: Option<Phase>,
    /// Microphone loudness 0..1 (RMS of the last chunk).
    pub level: f32,
    /// The input device the mic process reads, once it is running.
    pub mic_device: String,
    /// Why the mic is not running, when it failed to start or died.
    pub mic_error: Option<String>,
    /// The output device replies play through, once the first reply played.
    pub speaker_device: String,
    /// What the reply audio is saying, when the server tells us.
    pub reply: String,
    pub error: Option<String>,
    /// `session.ready.engine`: `duplex` answers on its own; `pipeline`
    /// (or an older server that says nothing) leaves replies to us.
    pub engine: String,
    /// Whether the server has a kernel behind it (`session.ready.kernel`).
    pub kernel: bool,
    /// `session.ready.reply`: who answers dictation on the server (`none`
    /// = nobody there; this window's kernel does).
    pub reply_backend: String,
    /// `session.ready.text`: who answers the text channel (`none` = nobody).
    pub text_backend: String,
    /// A call is live (`session.start {mode: "call"}` was answered with a
    /// narrator).
    pub call: bool,
    /// The mic is sending silence.
    pub muted: bool,
    /// The last line the narrator spoke (`narrator.say`).
    pub last_said: String,
}

/// Mirror lines kept when nobody drains them (a closed window).
const MIRROR_CAP: usize = 200;

#[derive(Default)]
struct Shared {
    phase: Option<Phase>,
    engine: String,
    kernel: bool,
    reply_backend: String,
    text_backend: String,
    /// The server's agent activity, oldest first, until the UI takes it.
    mirror: Vec<Mirror>,
    /// A `text.input` turn's reply as it streams.
    text_reply: String,
    finals: Vec<String>,
    partial: String,
    /// Set when the server closed the take (`transcript.final`).
    take_done: bool,
    reply: String,
    level: f32,
    mic_device: String,
    mic_error: Option<String>,
    speaker_device: String,
    error: Option<String>,
    /// Bytes of reply audio played, for the tests.
    played: u64,
    interrupts: u32,
    call: bool,
    muted: bool,
    last_said: String,
}

enum Cmd {
    MicStart,
    MicStop,
    Speak(String),
    Text(String),
    Interrupt,
    Mute(bool),
    End,
}

struct Session {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Mutex<Shared>>,
    cfg: VoiceCfg,
    kind: SessionKind,
}

fn hold() -> &'static Mutex<Option<Session>> {
    static HOLD: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    HOLD.get_or_init(|| Mutex::new(None))
}

/// Whether a speech server is configured (`voice_url` in config.toml).
pub fn configured() -> bool {
    crate::kernel::voice_config().is_some()
}

pub fn status() -> Peek {
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    let Some(session) = hold.as_ref() else {
        return Peek::default();
    };
    let s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
    let mut text = s.finals.join(" ");
    if !s.partial.is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&s.partial);
    }
    Peek {
        text,
        phase: s.phase,
        level: s.level,
        mic_device: s.mic_device.clone(),
        mic_error: s.mic_error.clone(),
        speaker_device: s.speaker_device.clone(),
        reply: s.reply.clone(),
        error: s.error.clone(),
        engine: s.engine.clone(),
        kernel: s.kernel,
        reply_backend: s.reply_backend.clone(),
        text_backend: s.text_backend.clone(),
        call: s.call,
        muted: s.muted,
        last_said: s.last_said.clone(),
    }
}

/// Whether a call is live right now.
pub fn in_call() -> bool {
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    hold.as_ref().is_some_and(|session| {
        matches!(session.kind, SessionKind::Call { .. }) && {
            let s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
            s.phase.is_some() && s.phase != Some(Phase::Off)
        }
    })
}

/// Call `project`: open a call session (a dictation session, if any, ends),
/// keep the mic open, and let the gateway's narrator speak. Blocks until the
/// gateway answers `session.ready` or the connect times out.
pub fn call_start(project: &str) -> Result<()> {
    let cfg = crate::kernel::voice_config().ok_or_else(|| anyhow!("no voice_url in config"))?;
    // No microphone program means a call that streams silence and hears
    // nothing back: refuse now, with the install hint, not after connecting.
    mic_command().map_err(|e| anyhow!("no microphone for the call: {e}"))?;
    let kind = SessionKind::Call {
        project: project.to_string(),
    };
    ensure_session(&cfg, &kind)?;
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    let session = hold.as_ref().ok_or_else(|| anyhow!("voice session closed"))?;
    {
        let mut s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
        if !s.call {
            bail!("the speech server did not open a call (session.ready.mode != call); does it have a kernel?");
        }
        s.finals.clear();
        s.partial.clear();
        s.take_done = false;
        s.error = None;
        s.phase = Some(Phase::Listening);
    }
    session
        .tx
        .send(Cmd::MicStart)
        .map_err(|_| anyhow!("voice session closed"))
}

/// Hang up. The session closes; the record of the call is in the chat.
pub fn call_end() {
    let mut hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(session) = hold.as_ref()
        && matches!(session.kind, SessionKind::Call { .. })
        && let Some(session) = hold.take()
    {
        let _ = session.tx.send(Cmd::End);
    }
}

/// Mute or unmute the mic during a call. The mic keeps running (the gateway
/// wants a continuous stream); muted frames are silence.
pub fn call_mute(muted: bool) -> Result<()> {
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    let session = hold.as_ref().ok_or_else(|| anyhow!("no call"))?;
    {
        let mut s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
        s.muted = muted;
    }
    session
        .tx
        .send(Cmd::Mute(muted))
        .map_err(|_| anyhow!("voice session closed"))
}

/// Take what the server's agent did since the last call.
pub fn drain_mirror() -> Vec<Mirror> {
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    let Some(session) = hold.as_ref() else {
        return Vec::new();
    };
    let mut s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
    std::mem::take(&mut s.mirror)
}

/// Send words to the server's own model over its text channel (`/voice
/// <text>` in the composer): it answers aloud and, when it has a kernel,
/// may hand the task to it. Connects when needed. Not a kernel prompt.
pub fn text_input(text: &str) -> Result<()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let cfg = crate::kernel::voice_config().ok_or_else(|| anyhow!("no voice_url in config"))?;
    ensure_session(&cfg, &SessionKind::Dictation)?;
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    let session = hold.as_ref().ok_or_else(|| anyhow!("voice session closed"))?;
    {
        let mut s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
        if s.call {
            // Typed during a call: the same inbox as the spoken words, filed as `text`.
            s.text_reply.clear();
            drop(s);
            return session
                .tx
                .send(Cmd::Text(text.to_string()))
                .map_err(|_| anyhow!("voice session closed"));
        }
        if s.text_backend == "none" {
            bail!(
                "the speech server has nobody to answer text (session.ready.text = none); set voice_reply = \"kernel\" or \"openrouter\" in config.toml"
            );
        }
        s.text_reply.clear();
        s.reply.clear();
    }
    session
        .tx
        .send(Cmd::Text(text.to_string()))
        .map_err(|_| anyhow!("voice session closed"))
}

/// Whether the connected server answers by itself (`engine: duplex`). A
/// dictated prompt must then not be sent to the kernel too, and the
/// kernel's answer must not be `speak`-ed: the user would hear two replies.
pub fn server_answers() -> bool {
    let p = status();
    p.engine == "duplex" || (!p.reply_backend.is_empty() && p.reply_backend != "none")
}

/// Bytes of reply audio handed to the player so far, and interrupts sent.
pub fn counters() -> (u64, u32) {
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    hold.as_ref()
        .map(|s| {
            let s = s.shared.lock().unwrap_or_else(|p| p.into_inner());
            (s.played, s.interrupts)
        })
        .unwrap_or((0, 0))
}

/// Open the mic. Connects first when no session is live. Blocks until the
/// server says `session.ready` or the connect times out.
pub fn start() -> Result<()> {
    let cfg = crate::kernel::voice_config().ok_or_else(|| anyhow!("no voice_url in config"))?;
    if in_call() {
        bail!("a call is live; the mic is already open");
    }
    ensure_session(&cfg, &SessionKind::Dictation)?;
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    let session = hold.as_ref().ok_or_else(|| anyhow!("voice session closed"))?;
    {
        let mut s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
        s.finals.clear();
        s.partial.clear();
        s.take_done = false;
        s.error = None;
        // Speaking into a reply is barge-in: the reply stops.
        if s.phase == Some(Phase::Speaking) {
            s.interrupts += 1;
            let _ = session.tx.send(Cmd::Interrupt);
        }
        s.phase = Some(Phase::Listening);
    }
    session
        .tx
        .send(Cmd::MicStart)
        .map_err(|_| anyhow!("voice session closed"))?;
    Ok(())
}

/// The words so far, for the composer's ghost text.
pub fn peek() -> Result<String> {
    let p = status();
    if let Some(e) = p.error {
        bail!("{e}");
    }
    Ok(p.text)
}

/// Close the mic and return the take. Waits a moment for the server's
/// final transcript; the partial stands when it does not come.
pub fn stop() -> Result<String> {
    let (tx, shared) = {
        let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
        let session = hold.as_ref().ok_or_else(|| anyhow!("no voice session"))?;
        (session.tx.clone(), Arc::clone(&session.shared))
    };
    let _ = tx.send(Cmd::MicStop);
    let deadline = Instant::now() + FINAL_WAIT;
    loop {
        {
            let s = shared.lock().unwrap_or_else(|p| p.into_inner());
            if s.take_done || s.error.is_some() || Instant::now() > deadline {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(e) = s.error.take() {
        s.phase = Some(Phase::Ready);
        bail!("{e}");
    }
    let mut text = s.finals.join(" ");
    if !s.take_done && !s.partial.is_empty() {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&s.partial);
    }
    s.partial.clear();
    s.finals.clear();
    if s.phase == Some(Phase::Listening) {
        s.phase = Some(Phase::Ready);
    }
    Ok(text.trim().to_string())
}

/// Voice this reply. Connects when needed; a reply already playing is cut.
pub fn speak(text: &str) -> Result<()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let cfg = crate::kernel::voice_config().ok_or_else(|| anyhow!("no voice_url in config"))?;
    if in_call() {
        return Ok(()); // the narrator speaks for the agent during a call
    }
    ensure_session(&cfg, &SessionKind::Dictation)?;
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    let session = hold.as_ref().ok_or_else(|| anyhow!("voice session closed"))?;
    {
        let mut s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
        s.reply.clear();
        s.phase = Some(Phase::Speaking);
    }
    session
        .tx
        .send(Cmd::Speak(text.to_string()))
        .map_err(|_| anyhow!("voice session closed"))
}

/// Stop the reply audio now. Harmless when nothing plays.
pub fn interrupt() {
    let hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(session) = hold.as_ref() {
        let mut s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
        if s.phase == Some(Phase::Speaking) {
            s.interrupts += 1;
            s.phase = Some(Phase::Ready);
            let _ = session.tx.send(Cmd::Interrupt);
        }
    }
}

/// Drop the session (config change, app quit).
pub fn shutdown() {
    let mut hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(session) = hold.take() {
        let _ = session.tx.send(Cmd::End);
    }
}

fn ensure_session(cfg: &VoiceCfg, kind: &SessionKind) -> Result<()> {
    {
        let mut hold = hold().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(session) = hold.as_ref() {
            let dead = {
                let s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
                s.phase.is_none() || s.phase == Some(Phase::Off)
            };
            if session.cfg == *cfg && session.kind == *kind && !dead {
                return Ok(());
            }
            if let Some(old) = hold.take() {
                let _ = old.tx.send(Cmd::End);
            }
        }
    }
    if !(cfg.url.starts_with("ws://") || cfg.url.starts_with("wss://")) {
        bail!("voice_url must start with ws:// or wss://, not {:?}", cfg.url);
    }
    let shared = Arc::new(Mutex::new(Shared {
        phase: Some(Phase::Connecting),
        ..Default::default()
    }));
    let (tx, rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
    {
        let shared = Arc::clone(&shared);
        let cfg = cfg.clone();
        let kind = kind.clone();
        crate::agent::acp::runtime().spawn(async move {
            if let Err(e) = run(cfg, kind, rx, Arc::clone(&shared), ready_tx).await {
                let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                s.error = Some(format!("voice server: {e:#}"));
            }
            let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
            s.phase = Some(Phase::Off);
        });
    }
    let mut hold = hold().lock().unwrap_or_else(|p| p.into_inner());
    *hold = Some(Session {
        tx,
        shared: Arc::clone(&shared),
        cfg: cfg.clone(),
        kind: kind.clone(),
    });
    drop(hold);
    match ready_rx.recv_timeout(CONNECT_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            shutdown();
            Err(e)
        }
        Err(_) => {
            shutdown();
            bail!("voice server {} did not answer in {CONNECT_TIMEOUT:?}", cfg.url)
        }
    }
}

/// The session task: one socket, the mic and player processes, the
/// commands from the UI thread.
/// rustls has two crypto backends in this binary (ring via one crate,
/// aws-lc-rs via another) and refuses to guess; `wss://` needs one named.
fn install_tls_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

async fn run(
    cfg: VoiceCfg,
    kind: SessionKind,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    shared: Arc<Mutex<Shared>>,
    ready: std::sync::mpsc::Sender<Result<()>>,
) -> Result<()> {
    install_tls_provider();
    let mut request = cfg
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| anyhow!("{}: {e}", cfg.url))?;
    if let Some(token) = cfg.token.as_deref().filter(|t| !t.is_empty()) {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}")
                .parse()
                .map_err(|_| anyhow!("voice_token has characters a header cannot carry"))?,
        );
    }
    let connected = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::connect_async(request),
    )
    .await;
    let (ws, _) = match connected {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            let msg = format!("cannot reach {}: {e}", cfg.url);
            let _ = ready.send(Err(anyhow!("{msg}")));
            bail!("{msg}");
        }
        Err(_) => {
            let msg = format!("{} did not answer the handshake", cfg.url);
            let _ = ready.send(Err(anyhow!("{msg}")));
            bail!("{msg}");
        }
    };
    let (mut sink, mut stream) = ws.split();
    let mut start = json!({
        "type": "session.start",
        "format": { "type": "audio/pcm", "rate": RATE },
        // Replies are ours to drive (`speak`) when the engine leaves them
        // to the client; the agent mirror is off — this window has the chat.
        "reply": if cfg.reply.is_empty() { "none" } else { cfg.reply.as_str() },
        "agents": cfg.mirror
    });
    if kind == SessionKind::Dictation {
        // Dictation: the gateway's streaming recogniser on any engine — partials
        // as the words come, a final on release, no reply, no speech model.
        start["mode"] = json!("dictation");
    }
    if let SessionKind::Call { project } = &kind {
        // A call: the caller's words go to the project's main agent as
        // `voice` inbox messages and the gateway's narrator speaks the
        // highlights. This window shows the chat, so "on your screen" fits.
        start["mode"] = json!("call");
        start["channel"] = json!("voice");
        start["device"] = json!("desktop");
        start["screen"] = json!("on your screen");
        if !project.is_empty() {
            start["project"] = json!(project);
        }
    }
    sink.send(text_frame(start)).await?;

    let (mic_tx, mut mic_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut mic: Option<Mic> = None;
    let mut player: Option<Player> = None;
    let mut ready_sent = false;
    let mut speaking = false;
    let mut muted = false;
    // Client-side echo gate state: when playback last ended, and how loud the
    // mic runs while we play (our own voice coming back).
    let mut gate_until = Instant::now();
    let mut echo_floor = 0.0f32;

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Cmd::MicStart => {
                        if mic.is_none() {
                            match Mic::spawn(mic_tx.clone(), Arc::clone(&shared)) {
                                Ok(m) => {
                                    mic = Some(m);
                                    shared.lock().unwrap_or_else(|p| p.into_inner()).mic_error = None;
                                }
                                Err(e) => {
                                    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                                    s.error = Some(format!("microphone: {e:#}"));
                                    s.mic_error = Some(format!("{e:#}"));
                                    s.phase = Some(Phase::Ready);
                                }
                            }
                        }
                    }
                    Cmd::MicStop => {
                        if let Some(m) = mic.take() {
                            m.stop();
                        }
                        let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                        s.level = 0.0;
                    }
                    Cmd::Speak(text) => {
                        if let Some(p) = player.take() {
                            p.stop();
                        }
                        speaking = true;
                        sink.send(text_frame(json!({ "type": "speak", "text": text }))).await?;
                    }
                    Cmd::Text(text) => {
                        sink.send(text_frame(json!({ "type": "text.input", "text": text })))
                            .await?;
                    }
                    Cmd::Interrupt => {
                        if let Some(p) = player.take() {
                            p.stop();
                        }
                        speaking = false;
                        sink.send(text_frame(json!({ "type": "interrupt" }))).await?;
                    }
                    Cmd::Mute(on) => {
                        muted = on;
                    }
                    Cmd::End => {
                        if let Some(m) = mic.take() { m.stop(); }
                        if let Some(p) = player.take() { p.stop(); }
                        let _ = sink.send(text_frame(json!({ "type": "session.end" }))).await;
                        let _ = sink.close().await;
                        break;
                    }
                }
            }
            chunk = mic_rx.recv() => {
                let Some(chunk) = chunk else { continue };
                if mic.is_some() {
                    // Muted: the stream keeps its clock, the words stay home.
                    // Playing: a bare speaker feeds the mic our own reply (no
                    // AEC on a plain CoreAudio stream); frames no louder than
                    // that echo go out as silence, a voice over it passes so
                    // barge-in still works. The gateway's gate does the rest.
                    let playing = speaking || Instant::now() < gate_until;
                    let level = rms(&chunk);
                    if playing {
                        echo_floor = if echo_floor == 0.0 { level } else { echo_floor * 0.9 + level * 0.1 };
                    }
                    let echo = playing && !(level > echo_floor * 2.5 && level > 0.03);
                    let chunk = if muted || echo { vec![0u8; chunk.len()] } else { chunk };
                    sink.send(Message::Binary(chunk.into())).await?;
                }
            }
            frame = stream.next() => {
                let Some(frame) = frame else {
                    bail!("connection to {} closed", cfg.url);
                };
                match frame? {
                    Message::Binary(pcm) => {
                        if !speaking {
                            continue;
                        }
                        if player.is_none() {
                            match Player::spawn(&shared) {
                                Ok(p) => player = Some(p),
                                Err(e) => {
                                    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                                    if s.error.is_none() {
                                        s.error = Some(format!("no audio output: {e:#}"));
                                    }
                                    speaking = false;
                                    continue;
                                }
                            }
                        }
                        if let Some(p) = player.as_mut() {
                            if p.write(&pcm).is_err() {
                                player = None;
                            } else {
                                let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                                s.played += pcm.len() as u64;
                            }
                        }
                    }
                    Message::Text(text) => {
                        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
                        let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                        let mut send_interrupt = false;
                        let mut say_speaking: Option<bool> = None;
                        {
                        let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                        match kind {
                            "session.ready" => {
                                s.engine = field("engine");
                                s.kernel = v.get("kernel").and_then(Value::as_bool).unwrap_or(false);
                                s.reply_backend = field("reply");
                                s.text_backend = field("text");
                                s.call = field("mode") == "call"
                                    && v.get("narrator").and_then(Value::as_bool).unwrap_or(false);
                                if s.phase == Some(Phase::Connecting) {
                                    s.phase = Some(Phase::Ready);
                                }
                                if !ready_sent {
                                    ready_sent = true;
                                    let _ = ready.send(Ok(()));
                                }
                            }
                            "speech.started" => {
                                // The user talks over the reply: cut it.
                                if speaking {
                                    speaking = false;
                                    s.interrupts += 1;
                                    if let Some(p) = player.take() { p.stop(); }
                                    if s.phase == Some(Phase::Speaking) {
                                        s.phase = Some(Phase::Listening);
                                    }
                                    send_interrupt = true;
                                }
                            }
                            "speech.stopped" => {}
                            // An increment: append. (The Swift comment reads
                            // as a replacement; the server's protocol text says
                            // increment, and the live server sends increments.)
                            "transcript.delta" => s.partial.push_str(&field("text")),
                            // The whole line, replacing the open one. Empty =
                            // nothing was said; back to listening.
                            "transcript.final" => {
                                let text = field("text");
                                s.partial.clear();
                                if s.call {
                                    // A call has no take: the strip shows the last utterance only.
                                    s.finals.clear();
                                }
                                if !text.trim().is_empty() {
                                    s.finals.push(text.trim().to_string());
                                }
                                s.take_done = true;
                            }
                            // A duplex server starts replies on its own; play
                            // them like our own `speak`.
                            "response.started" => {
                                if !speaking {
                                    speaking = true;
                                    s.reply.clear();
                                }
                                s.phase = Some(Phase::Speaking);
                                // The gateway's echo gate tightens while we play.
                                say_speaking = Some(true);
                            }
                            // Increments with their own spacing: append raw.
                            "response.transcript" => s.reply.push_str(&field("text")),
                            "response.done" => {
                                speaking = false;
                                let interrupted = v.get("interrupted").and_then(Value::as_bool).unwrap_or(false);
                                if let Some(p) = player.take() {
                                    // Cut short: nothing queued should still be heard.
                                    if interrupted { p.stop() } else { p.finish() }
                                }
                                if s.phase == Some(Phase::Speaking) {
                                    s.phase = Some(if mic.is_some() { Phase::Listening } else { Phase::Ready });
                                }
                                gate_until = Instant::now() + ECHO_TAIL;
                                say_speaking = Some(false);
                            }
                            // The text channel: the reply streams into the
                            // status row like a spoken one, and lands in the
                            // chat whole when done.
                            "text.delta" => {
                                let t = field("text");
                                s.text_reply.push_str(&t);
                                s.reply.push_str(&t);
                            }
                            // The narrator is about to speak this line: into
                            // the chat as a `voice ·` line, and onto the call strip.
                            "narrator.say" => {
                                let text = field("text");
                                s.last_said = text.clone();
                                push_mirror(&mut s, Mirror { kind: format!("narrator.say/{}", field("kind")), agent: field("ref"), text });
                            }
                            "text.done" if v.get("forwarded").and_then(Value::as_bool).unwrap_or(false) => {
                                s.text_reply.clear();
                            }
                            "text.done" => {
                                let whole = field("text");
                                let text = if whole.is_empty() { std::mem::take(&mut s.text_reply) } else { whole };
                                s.text_reply.clear();
                                let cancelled = v.get("cancelled").and_then(Value::as_bool).unwrap_or(false);
                                push_mirror(&mut s, Mirror {
                                    kind: "text.done".into(),
                                    agent: String::new(),
                                    text: if cancelled { format!("{text} (cancelled)") } else { text },
                                });
                            }
                            // The server's agent, mirrored: what it says, what
                            // it runs, when it is done. `agent.tree` is not
                            // shown (this window has its own tree).
                            "agent.event" => {
                                let kind = field("kind");
                                let from = field("from");
                                let text = field("text");
                                if kind == "assistant" && text.trim().is_empty() {
                                    // Per-token deltas for the root; the
                                    // whole answer arrives as agent.done.
                                } else if kind != "assistant" {
                                    let text = if from.is_empty() { text } else { format!("{from}: {text}") };
                                    push_mirror(&mut s, Mirror { kind: format!("agent.event/{kind}"), agent: field("agent"), text });
                                }
                            }
                            "agent.done" => push_mirror(&mut s, Mirror { kind: "agent.done".into(), agent: field("agent"), text: field("text") }),
                            "agent.turn" => push_mirror(&mut s, Mirror { kind: "agent.turn".into(), agent: field("agent"), text: field("state") }),
                            "tool.call" => {
                                let args = v.get("arguments").map(|a| a.to_string()).unwrap_or_default();
                                let args: String = args.chars().take(120).collect();
                                push_mirror(&mut s, Mirror { kind: "tool.call".into(), agent: String::new(), text: format!("{} {args}", field("name")) });
                            }
                            "tool.result" => {
                                let out: String = field("output").chars().take(200).collect();
                                push_mirror(&mut s, Mirror { kind: "tool.result".into(), agent: String::new(), text: format!("{} → {out}", field("name")) });
                            }
                            "agent.tree" => {}
                            "error" => {
                                let m = field("message");
                                s.error = Some(if m.is_empty() { "voice server error".into() } else { m });
                            }
                            _ => {}
                        }
                        }
                        if send_interrupt {
                            sink.send(text_frame(json!({ "type": "interrupt" }))).await?;
                        }
                        if let Some(on) = say_speaking {
                            sink.send(text_frame(json!({ "type": "client.speaking", "speaking": on }))).await?;
                        }
                    }
                    Message::Close(_) => bail!("{} closed the session", cfg.url),
                    _ => {}
                }
            }
        }
    }
    if !ready_sent {
        let _ = ready.send(Err(anyhow!("session ended before ready")));
    }
    Ok(())
}

fn text_frame(v: Value) -> Message {
    Message::Text(Utf8Bytes::from(v.to_string()))
}

/// The microphone.
///
/// On macOS it is CoreAudio inside this process ([`native`]): the capture —
/// and so the Privacy › Microphone prompt and its row — belong to the app,
/// not to a child program macOS may attribute elsewhere or fail to find.
/// Elsewhere, or when `ARBOS_VOICE_MIC_CMD` names one, a process writing raw
/// PCM16 mono 24 kHz to stdout, read on a thread in 100 ms chunks.
enum Mic {
    #[cfg(target_os = "macos")]
    Native(native::Capture),
    Process(Child),
}

impl Mic {
    fn spawn(tx: mpsc::UnboundedSender<Vec<u8>>, shared: Arc<Mutex<Shared>>) -> Result<Self> {
        #[cfg(target_os = "macos")]
        if std::env::var("ARBOS_VOICE_MIC_CMD")
            .ok()
            .is_none_or(|c| c.trim().is_empty())
        {
            // Opening the device would ask too; asking first makes the
            // dialog the app's own even when the device open fails.
            request_mic_permission();
            match native::Capture::start(tx.clone(), Arc::clone(&shared)) {
                Ok(capture) => return Ok(Self::Native(capture)),
                Err(e) => {
                    // A program can still do it; the strip says why the
                    // native path did not.
                    eprintln!("voice: native microphone unavailable ({e:#}); trying a program");
                }
            }
        }
        Self::spawn_process(tx, shared)
    }

    fn spawn_process(
        tx: mpsc::UnboundedSender<Vec<u8>>,
        shared: Arc<Mutex<Shared>>,
    ) -> Result<Self> {
        let mut cmd = mic_command()?;
        // What the strip shows as `mic: …`: the device when we chose one,
        // else the program's name.
        let device = {
            let args: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            args.iter()
                .find(|a| a.starts_with(':') && a.len() > 1)
                .map(|a| a[1..].to_string())
                .unwrap_or_else(|| {
                    std::path::Path::new(cmd.get_program())
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
        };
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("start {}: {e}", cmd.get_program().to_string_lossy()))?;
        shared.lock().unwrap().mic_device = device;
        let mut out = child.stdout.take().ok_or_else(|| anyhow!("mic has no stdout"))?;
        std::thread::Builder::new()
            .name("arbos-mic".into())
            .spawn(move || {
                let mut buf = vec![0u8; CHUNK];
                let mut filled = 0;
                loop {
                    match out.read(&mut buf[filled..]) {
                        Ok(0) => break,
                        Ok(n) => {
                            filled += n;
                            if filled == CHUNK {
                                {
                                    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                                    s.level = rms(&buf);
                                }
                                if tx.send(buf.clone()).is_err() {
                                    break;
                                }
                                filled = 0;
                            }
                        }
                        Err(_) => break,
                    }
                }
                if filled > 0 {
                    let _ = tx.send(buf[..filled].to_vec());
                }
                // The program ended on its own: the device is gone or busy.
                // The strip shows it instead of a level that never moves.
                let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                if s.mic_error.is_none() && s.phase.is_some_and(|p| p != Phase::Off) {
                    s.mic_error = Some("the microphone program stopped".into());
                }
                s.level = 0.0;
            })
            .map_err(|e| anyhow!("mic thread: {e}"))?;
        Ok(Self::Process(child))
    }

    fn stop(self) {
        match self {
            #[cfg(target_os = "macos")]
            Self::Native(capture) => capture.stop(),
            Self::Process(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// The microphone over CoreAudio, in this process.
///
/// cpal's stream is not `Send`, so it lives on its own thread: the thread
/// opens the default input device (or the one `ARBOS_VOICE_MIC_DEVICE`
/// names), reports the outcome, then sleeps until told to stop. Opening the
/// device is what makes macOS ask for the microphone the first time, in the
/// app's name, with `NSMicrophoneUsageDescription` as the reason.
///
/// The device's own format (usually 48 kHz, one or two channels, f32) is
/// mixed to mono and resampled to 24 kHz PCM16 here, in 100 ms chunks like
/// the program path, so the rest of the session sees no difference.
#[cfg(target_os = "macos")]
mod native {
    use super::{CHUNK, RATE, Shared, rms};
    use anyhow::{Result, anyhow, bail};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{Arc, Mutex, mpsc as sync_mpsc};
    use std::thread::JoinHandle;
    use tokio::sync::mpsc;

    pub struct Capture {
        /// Dropping this (or sending on it) ends the thread and the stream.
        stop: sync_mpsc::Sender<()>,
        thread: Option<JoinHandle<()>>,
    }

    impl Capture {
        pub fn start(tx: mpsc::UnboundedSender<Vec<u8>>, shared: Arc<Mutex<Shared>>) -> Result<Self> {
            let (stop, stop_rx) = sync_mpsc::channel::<()>();
            let (ready, ready_rx) = sync_mpsc::channel::<Result<String>>();
            let thread_shared = Arc::clone(&shared);
            let thread = std::thread::Builder::new()
                .name("arbos-mic".into())
                .spawn(move || {
                    let stream = match open(tx, thread_shared) {
                        Ok((stream, name)) => {
                            let _ = ready.send(Ok(name));
                            stream
                        }
                        Err(e) => {
                            let _ = ready.send(Err(e));
                            return;
                        }
                    };
                    // Err means the handle was dropped: stop all the same.
                    let _ = stop_rx.recv();
                    drop(stream);
                })
                .map_err(|e| anyhow!("mic thread: {e}"))?;
            let name = ready_rx
                .recv()
                .map_err(|_| anyhow!("mic thread ended before opening the device"))??;
            shared.lock().unwrap_or_else(|p| p.into_inner()).mic_device = name;
            Ok(Self {
                stop,
                thread: Some(thread),
            })
        }

        pub fn stop(mut self) {
            let _ = self.stop.send(());
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn pick_device(host: &cpal::Host) -> Result<cpal::Device> {
        if let Some(wanted) = std::env::var("ARBOS_VOICE_MIC_DEVICE")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            let mut devices = host
                .input_devices()
                .map_err(|e| anyhow!("list input devices: {e}"))?;
            return devices
                .find(|d| d.name().is_ok_and(|n| n == wanted))
                .ok_or_else(|| anyhow!("no input device named {wanted:?}"));
        }
        host.default_input_device()
            .ok_or_else(|| anyhow!("no default input device"))
    }

    fn open(
        tx: mpsc::UnboundedSender<Vec<u8>>,
        shared: Arc<Mutex<Shared>>,
    ) -> Result<(cpal::Stream, String)> {
        let host = cpal::default_host();
        let device = pick_device(&host)?;
        let name = device.name().unwrap_or_else(|_| "microphone".into());
        let config = device
            .default_input_config()
            .map_err(|e| anyhow!("{name}: no input format: {e}"))?;
        let mut conv = Converter::new(config.channels() as usize, config.sample_rate().0, tx, Arc::clone(&shared));
        let on_error = move |e: cpal::StreamError| {
            let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
            s.mic_error = Some(format!("microphone stream: {e}"));
            s.level = 0.0;
        };
        let stream_config: cpal::StreamConfig = config.clone().into();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| conv.push(data.iter().copied()),
                on_error,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| conv.push(data.iter().map(|&v| v as f32 / 32768.0)),
                on_error,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    conv.push(data.iter().map(|&v| (v as f32 - 32768.0) / 32768.0))
                },
                on_error,
                None,
            ),
            other => bail!("{name}: unsupported sample format {other:?}"),
        }
        .map_err(|e| anyhow!("{name}: open input stream: {e}"))?;
        stream
            .play()
            .map_err(|e| anyhow!("{name}: start capture: {e}"))?;
        Ok((stream, name))
    }

    /// Device frames in, 24 kHz mono PCM16 chunks out.
    struct Converter {
        channels: usize,
        /// Input samples per output sample.
        step: f64,
        /// Read position in `mono`, fractional.
        pos: f64,
        mono: Vec<f32>,
        out: Vec<u8>,
        tx: mpsc::UnboundedSender<Vec<u8>>,
        shared: Arc<Mutex<Shared>>,
    }

    impl Converter {
        fn new(
            channels: usize,
            in_rate: u32,
            tx: mpsc::UnboundedSender<Vec<u8>>,
            shared: Arc<Mutex<Shared>>,
        ) -> Self {
            Self {
                channels: channels.max(1),
                step: in_rate as f64 / RATE as f64,
                pos: 0.0,
                mono: Vec::new(),
                out: Vec::new(),
                tx,
                shared,
            }
        }

        fn push(&mut self, samples: impl Iterator<Item = f32>) {
            let mut acc = 0f32;
            let mut n = 0usize;
            for v in samples {
                acc += v;
                n += 1;
                if n == self.channels {
                    self.mono.push(acc / self.channels as f32);
                    acc = 0.0;
                    n = 0;
                }
            }
            // Linear interpolation: plenty for speech going to a 24 kHz
            // recogniser, and no filter state to get wrong.
            while (self.pos as usize) + 1 < self.mono.len() {
                let i = self.pos as usize;
                let f = (self.pos - i as f64) as f32;
                let v = self.mono[i] * (1.0 - f) + self.mono[i + 1] * f;
                let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                self.out.extend_from_slice(&s.to_le_bytes());
                self.pos += self.step;
            }
            let consumed = (self.pos as usize).min(self.mono.len());
            if consumed > 0 {
                self.mono.drain(..consumed);
                self.pos -= consumed as f64;
            }
            while self.out.len() >= CHUNK {
                let chunk: Vec<u8> = self.out.drain(..CHUNK).collect();
                {
                    let mut s = self.shared.lock().unwrap_or_else(|p| p.into_inner());
                    s.level = rms(&chunk);
                }
                let _ = self.tx.send(chunk);
            }
        }
    }
}

/// Whether macOS lets this app hear the microphone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicPermission {
    /// Never asked: the next capture (or [`request_mic_permission`]) puts
    /// the dialog on screen.
    NotDetermined,
    Restricted,
    /// Jacob said no, or turned it off in Privacy & Security › Microphone.
    Denied,
    Authorized,
    /// Not macOS: nothing to ask.
    NotApplicable,
}

impl MicPermission {
    /// What Settings tells the user to do, when something is in the way.
    pub fn advice(self) -> Option<&'static str> {
        match self {
            Self::Denied | Self::Restricted => Some(
                "microphone is off for Arbos: System Settings › Privacy & Security › Microphone, turn on Arbos",
            ),
            Self::NotDetermined => Some("macOS will ask: click Allow"),
            Self::Authorized | Self::NotApplicable => None,
        }
    }
}

/// `AVCaptureDevice.authorizationStatusForMediaType:` — the same row
/// System Settings shows, read from inside the app.
#[cfg(target_os = "macos")]
pub fn mic_permission() -> MicPermission {
    permission::status()
}

#[cfg(not(target_os = "macos"))]
pub fn mic_permission() -> MicPermission {
    MicPermission::NotApplicable
}

/// Ask macOS for the microphone from this process, once. The first call
/// puts the system dialog on screen and gives the app its row under
/// Privacy & Security › Microphone; later calls return at once. Nothing
/// is captured. Safe to call from any thread.
pub fn request_mic_permission() {
    #[cfg(target_os = "macos")]
    permission::request();
}

#[cfg(target_os = "macos")]
mod permission {
    use super::MicPermission;
    use block::ConcreteBlock;
    use objc::runtime::{BOOL, Object};
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        static AVMediaTypeAudio: *const Object;
    }

    pub fn status() -> MicPermission {
        // AVAuthorizationStatus: notDetermined 0, restricted 1, denied 2,
        // authorized 3.
        let code: i64 = unsafe {
            msg_send![
                class!(AVCaptureDevice),
                authorizationStatusForMediaType: AVMediaTypeAudio
            ]
        };
        match code {
            0 => MicPermission::NotDetermined,
            1 => MicPermission::Restricted,
            2 => MicPermission::Denied,
            _ => MicPermission::Authorized,
        }
    }

    pub fn request() {
        if status() != MicPermission::NotDetermined {
            return;
        }
        let block = ConcreteBlock::new(move |granted: BOOL| {
            eprintln!(
                "voice: microphone permission {}",
                if granted == objc::runtime::YES { "granted" } else { "denied" }
            );
        })
        .copy();
        unsafe {
            let _: () = msg_send![
                class!(AVCaptureDevice),
                requestAccessForMediaType: AVMediaTypeAudio
                completionHandler: &*block
            ];
        }
    }
}

/// A snapshot of the Settings › Test mic probe.
#[derive(Debug, Clone, Default)]
pub struct MicTest {
    pub device: String,
    /// Loudness 0..1 of the last 100 ms.
    pub level: f32,
    pub error: Option<String>,
}

/// The Test mic probe: the same capture a take or a call uses, with nobody
/// listening to the audio — only the level is read. Starting it is also
/// what makes macOS ask for the microphone the first time.
struct MicProbe {
    mic: Option<Mic>,
    shared: Arc<Mutex<Shared>>,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    since: Instant,
}

/// A probe left running is stopped on its own after this.
const MIC_TEST_FOR: Duration = Duration::from_secs(60);

fn probe() -> &'static Mutex<Option<MicProbe>> {
    static PROBE: OnceLock<Mutex<Option<MicProbe>>> = OnceLock::new();
    PROBE.get_or_init(|| Mutex::new(None))
}

pub fn mic_test_start() {
    let shared = Arc::new(Mutex::new(Shared {
        phase: Some(Phase::Listening),
        ..Shared::default()
    }));
    let (tx, rx) = mpsc::unbounded_channel();
    let mic = match Mic::spawn(tx, Arc::clone(&shared)) {
        Ok(mic) => Some(mic),
        Err(e) => {
            shared.lock().unwrap_or_else(|p| p.into_inner()).mic_error = Some(format!("{e:#}"));
            None
        }
    };
    let previous = probe().lock().unwrap_or_else(|p| p.into_inner()).replace(MicProbe {
        mic,
        shared,
        rx,
        since: Instant::now(),
    });
    if let Some(p) = previous.and_then(|p| p.mic) {
        p.stop();
    }
}

pub fn mic_test_stop() {
    let taken = probe().lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(mic) = taken.and_then(|p| p.mic) {
        mic.stop();
    }
}

/// What the probe hears now; None when no test is running.
pub fn mic_test() -> Option<MicTest> {
    let mut guard = probe().lock().unwrap_or_else(|p| p.into_inner());
    let p = guard.as_mut()?;
    if p.since.elapsed() > MIC_TEST_FOR {
        drop(guard);
        mic_test_stop();
        return None;
    }
    // Nobody wants the audio; keep the channel from growing.
    while p.rx.try_recv().is_ok() {}
    let s = p.shared.lock().unwrap_or_else(|p| p.into_inner());
    Some(MicTest {
        device: s.mic_device.clone(),
        level: s.level,
        error: s.mic_error.clone(),
    })
}

/// The speaker. In-process through cpal (CoreAudio on a Mac, ALSA or
/// PipeWire on Linux) on the default output device, the way the phone
/// plays: no player program to find, no pipe to fill. `ARBOS_VOICE_PLAYER_CMD`
/// (tests, machines with no sound device) keeps the process backend: a
/// command reading raw PCM16 mono 24 kHz from stdin.
enum Player {
    Device(DeviceOut),
    Process(Child),}

/// Reply audio arrives as PCM16 mono 24 kHz; the device wants its own rate
/// and channel count. Samples are resampled linearly into a queue the
/// output callback drains; the queue is the whole state, so stop = clear.
struct DeviceOut {
    /// The cpal stream is not `Send`: it lives on its own thread, which
    /// holds it open while this flag is set.
    alive: Arc<std::sync::atomic::AtomicBool>,
    queue: Arc<Mutex<std::collections::VecDeque<f32>>>,
    /// Device sample rate and channels.
    rate: u32,
    channels: u16,
    /// Resampler carry: the last input sample and the fractional position.
    last: f32,
    pos: f64,
    /// Peak follower for the normaliser: reply speech lands at about the
    /// same loudness whatever the voice, like the phone's playback.
    peak: f32,
    gain: f32,
}

impl Drop for DeviceOut {
    fn drop(&mut self) {
        // Whatever path let go of the speaker, the thread must not outlive it.
        self.alive.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Reply speech is brought to this peak (about -6 dBFS); quiet voices are
/// lifted at most this much.
const TARGET_PEAK: f32 = 0.5;
const MAX_GAIN: f32 = 4.0;

impl Player {
    fn spawn(shared: &Arc<Mutex<Shared>>) -> Result<Self> {
        if let Some(custom) = std::env::var("ARBOS_VOICE_PLAYER_CMD")
            .ok()
            .filter(|c| !c.trim().is_empty())
        {
            let mut cmd = sh(&custom);
            let child = cmd
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| anyhow!("start {}: {e}", cmd.get_program().to_string_lossy()))?;
            shared.lock().unwrap_or_else(|p| p.into_inner()).speaker_device = "command".into();
            return Ok(Self::Process(child));
        }
        match DeviceOut::open() {
            Ok((out, name)) => {
                shared.lock().unwrap_or_else(|p| p.into_inner()).speaker_device = name;
                Ok(Self::Device(out))
            }
            Err(e) => {
                // No device (a headless box): a player program if there is one.
                let mut cmd = player_command().map_err(|e2| anyhow!("{e:#}; {e2:#}"))?;
                let child = cmd
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| anyhow!("start {}: {e}", cmd.get_program().to_string_lossy()))?;
                shared.lock().unwrap_or_else(|p| p.into_inner()).speaker_device =
                    std::path::Path::new(cmd.get_program())
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                Ok(Self::Process(child))
            }
        }
    }

    fn write(&mut self, pcm: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Device(out) => {
                out.push(pcm);
                Ok(())
            }
            Self::Process(child) => match child.stdin.as_mut() {
                Some(stdin) => stdin.write_all(pcm),
                None => Err(std::io::Error::other("player stdin closed")),
            },
        }
    }

    /// The reply is complete: let what is queued play out.
    fn finish(self) {
        match self {
            Self::Device(out) => {
                // The stream keeps running until the queue is empty, then ends.
                let queue = Arc::clone(&out.queue);
                let alive = Arc::clone(&out.alive);
                let per_second = (out.rate.max(1) * out.channels.max(1) as u32) as f64;
                std::thread::spawn(move || {
                    loop {
                        let left = queue.lock().unwrap_or_else(|p| p.into_inner()).len();
                        if left == 0 {
                            break;
                        }
                        std::thread::sleep(Duration::from_secs_f64((left as f64 / per_second).min(0.25)));
                    }
                    alive.store(false, std::sync::atomic::Ordering::Relaxed);
                });
            }
            Self::Process(mut child) => {
                drop(child.stdin.take());
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
        }
    }

    /// Barge-in: stop the sound now.
    fn stop(self) {
        match self {
            Self::Device(out) => {
                out.queue.lock().unwrap_or_else(|p| p.into_inner()).clear();
                out.alive.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            Self::Process(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl DeviceOut {
    fn open() -> Result<(Self, String)> {
        let queue: Arc<Mutex<std::collections::VecDeque<f32>>> =
            Arc::new(Mutex::new(std::collections::VecDeque::with_capacity(48_000)));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(String, u32, u16)>>();
        let q = Arc::clone(&queue);
        let flag = Arc::clone(&alive);
        std::thread::Builder::new()
            .name("arbos-speaker".into())
            .spawn(move || Self::run(q, flag, ready_tx))
            .map_err(|e| anyhow!("speaker thread: {e}"))?;
        let (name, rate, channels) = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("speaker did not open in time"))??;
        Ok((
            Self {
                alive,
                queue,
                rate,
                channels,
                last: 0.0,
                pos: 0.0,
                peak: TARGET_PEAK,
                gain: 1.0,
            },
            name,
        ))
    }

    /// The speaker thread: opens the default output, reports what it opened,
    /// plays the queue until `alive` goes false, then closes the stream.
    fn run(
        queue: Arc<Mutex<std::collections::VecDeque<f32>>>,
        alive: Arc<std::sync::atomic::AtomicBool>,
        ready: std::sync::mpsc::Sender<Result<(String, u32, u16)>>,
    ) {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let opened = (|| -> Result<(cpal::Stream, String, u32, u16)> {
            let host = cpal::default_host();
            let device = host
                .default_output_device()
                .ok_or_else(|| anyhow!("no default output device"))?;
            let name = device.name().unwrap_or_else(|_| "speaker".into());
            let config = device
                .default_output_config()
                .map_err(|e| anyhow!("output config: {e}"))?;
            let rate = config.sample_rate().0;
            let channels = config.channels();
            let q = Arc::clone(&queue);
            let stream = device
                .build_output_stream(
                    &config.config(),
                    move |data: &mut [f32], _| {
                        let mut q = q.lock().unwrap_or_else(|p| p.into_inner());
                        for sample in data.iter_mut() {
                            *sample = q.pop_front().unwrap_or(0.0);
                        }
                    },
                    |err| eprintln!("voice speaker: {err}"),
                    None,
                )
                .map_err(|e| anyhow!("output stream: {e}"))?;
            stream.play().map_err(|e| anyhow!("play: {e}"))?;
            Ok((stream, name, rate, channels))
        })();
        match opened {
            Err(e) => {
                let _ = ready.send(Err(e));
            }
            Ok((stream, name, rate, channels)) => {
                let _ = ready.send(Ok((name, rate, channels)));
                while alive.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                drop(stream);
            }
        }
    }

    /// PCM16 mono 24 kHz in; normalised, resampled, channel-duplicated
    /// samples onto the queue.
    fn push(&mut self, pcm: &[u8]) {
        let input: Vec<f32> = pcm
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect();
        if input.is_empty() {
            return;
        }
        // Normaliser: follow the peak (fast up, slow down), aim it at the target.
        let chunk_peak = input.iter().fold(0f32, |m, s| m.max(s.abs()));
        self.peak = if chunk_peak > self.peak {
            chunk_peak
        } else {
            self.peak * 0.995 + chunk_peak * 0.005
        };
        let want = (TARGET_PEAK / self.peak.max(1e-3)).clamp(0.25, MAX_GAIN);
        self.gain = self.gain * 0.9 + want * 0.1;
        let step = RATE as f64 / self.rate as f64;
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        while self.pos < input.len() as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i.min(input.len() - 1)];
            let s = ((a + (b - a) * frac) * self.gain).clamp(-1.0, 1.0);
            for _ in 0..self.channels {
                q.push_back(s);
            }
            self.pos += step;
        }
        self.pos -= input.len() as f64;
        self.last = *input.last().unwrap_or(&0.0);
    }
}

fn rms(pcm: &[u8]) -> f32 {
    let mut sum = 0f64;
    let mut n = 0usize;
    for pair in pcm.chunks_exact(2) {
        let v = i16::from_le_bytes([pair[0], pair[1]]) as f64 / 32768.0;
        sum += v * v;
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    ((sum / n as f64).sqrt() as f32 * 4.0).min(1.0)
}

fn which(program: &str) -> bool {
    find_program(program).is_some()
}

/// Where `program` is. A window launched from the Dock or `open` gets
/// macOS's minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), which has no
/// Homebrew in it, so the usual install prefixes are searched as well.
fn find_program(program: &str) -> Option<std::path::PathBuf> {
    let from_path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(program))
            .find(|f| f.is_file())
    });
    from_path.or_else(|| {
        ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"]
            .iter()
            .map(|d| std::path::Path::new(d).join(program))
            .find(|f| f.is_file())
    })
}

/// The system's default input device, by the name AVFoundation lists it
/// under. `ARBOS_VOICE_MIC_DEVICE` names one by hand. None: let ffmpeg
/// take audio device 0 — which on a Mac with virtual devices (RØDE
/// Connect, BlackHole) is often not a microphone at all.
#[cfg(target_os = "macos")]
fn default_input_device() -> Option<String> {
    if let Ok(name) = std::env::var("ARBOS_VOICE_MIC_DEVICE") {
        let name = name.trim().to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    let out = Command::new("/usr/sbin/system_profiler")
        .args(["SPAudioDataType", "-json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    v.get("SPAudioDataType")?
        .as_array()?
        .iter()
        .filter_map(|g| g.get("_items")?.as_array())
        .flatten()
        .find(|it| {
            it.get("coreaudio_default_audio_input_device")
                .and_then(Value::as_str)
                == Some("spaudio_yes")
        })
        .and_then(|it| it.get("_name")?.as_str().map(str::to_string))
}

#[cfg(not(target_os = "macos"))]
fn default_input_device() -> Option<String> {
    std::env::var("ARBOS_VOICE_MIC_DEVICE")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn sh(cmd: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd);
    c
}

/// The first microphone program found. `ARBOS_VOICE_MIC_CMD` (a shell
/// command writing raw PCM16 24 kHz mono to stdout) wins.
/// The capture program the mic would run, by name, or why there is none —
/// what the Microphone row in Settings › Permissions reads on Linux.
pub fn mic_program() -> Result<String> {
    let cmd = mic_command()?;
    Ok(cmd.get_program().to_string_lossy().into_owned())
}

fn mic_command() -> Result<Command> {
    if let Some(custom) = std::env::var("ARBOS_VOICE_MIC_CMD")
        .ok()
        .filter(|c| !c.trim().is_empty())
    {
        return Ok(sh(&custom));
    }
    let rate = RATE.to_string();
    if which("pw-record") {
        let mut c = Command::new("pw-record");
        c.args(["--format", "s16", "--rate", &rate, "--channels", "1", "-"]);
        return Ok(c);
    }
    if which("parec") {
        let mut c = Command::new("parec");
        c.args([
            "--format=s16le",
            &format!("--rate={rate}"),
            "--channels=1",
            "--raw",
        ]);
        return Ok(c);
    }
    if which("arecord") {
        let mut c = Command::new("arecord");
        c.args(["-q", "-f", "S16_LE", "-r", &rate, "-c", "1", "-t", "raw", "-"]);
        return Ok(c);
    }
    if which("rec") {
        let mut c = Command::new("rec");
        c.args([
            "-q", "-t", "raw", "-r", &rate, "-e", "signed", "-b", "16", "-c", "1", "-",
        ]);
        return Ok(c);
    }
    if cfg!(target_os = "macos") && let Some(ffmpeg) = find_program("ffmpeg") {
        let device = default_input_device().unwrap_or_else(|| "0".to_string());
        let mut c = Command::new(ffmpeg);
        c.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "avfoundation",
            "-i",
            &format!(":{device}"),
            "-ac",
            "1",
            "-ar",
            &rate,
            "-f",
            "s16le",
            "-",
        ]);
        return Ok(c);
    }
    bail!(
        "no microphone program found: install pipewire (pw-record), pulseaudio-utils (parec), alsa-utils (arecord) or sox (rec)"
    )
}

/// The first playback program found. `ARBOS_VOICE_PLAYER_CMD` (a shell
/// command reading raw PCM16 24 kHz mono from stdin) wins.
fn player_command() -> Result<Command> {
    if let Some(custom) = std::env::var("ARBOS_VOICE_PLAYER_CMD")
        .ok()
        .filter(|c| !c.trim().is_empty())
    {
        return Ok(sh(&custom));
    }
    let rate = RATE.to_string();
    if which("pw-play") {
        let mut c = Command::new("pw-play");
        c.args(["--format", "s16", "--rate", &rate, "--channels", "1", "-"]);
        return Ok(c);
    }
    if which("paplay") {
        let mut c = Command::new("paplay");
        c.args([
            "--raw",
            "--format=s16le",
            &format!("--rate={rate}"),
            "--channels=1",
        ]);
        return Ok(c);
    }
    if which("aplay") {
        let mut c = Command::new("aplay");
        c.args(["-q", "-f", "S16_LE", "-r", &rate, "-c", "1", "-t", "raw", "-"]);
        return Ok(c);
    }
    if which("play") {
        let mut c = Command::new("play");
        c.args([
            "-q", "-t", "raw", "-r", &rate, "-e", "signed", "-b", "16", "-c", "1", "-",
        ]);
        return Ok(c);
    }
    if which("ffplay") {
        let mut c = Command::new("ffplay");
        c.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nodisp",
            "-autoexit",
            "-f",
            "s16le",
            "-ar",
            &rate,
            "-ac",
            "1",
            "-i",
            "-",
        ]);
        return Ok(c);
    }
    bail!(
        "no playback program found: install pipewire (pw-play), pulseaudio-utils (paplay), alsa-utils (aplay), sox (play) or ffmpeg (ffplay)"
    )
}

/// Queue a mirror line, dropping the oldest past the cap.
fn push_mirror(s: &mut Shared, m: Mirror) {
    if s.mirror.len() >= MIRROR_CAP {
        s.mirror.remove(0);
    }
    s.mirror.push(m);
}
