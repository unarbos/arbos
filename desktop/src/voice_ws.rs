//! Voice over the self-hosted speech server.
//!
//! One WebSocket, the protocol of `ios/Arbos/Voice/SelfHostedVoiceSession.swift`:
//! binary frames are PCM16 mono 24 kHz both ways; JSON text frames carry
//! control. The server does speech only — the reply text comes from the
//! kernel and is handed back here with [`speak`].
//!
//! Microphone and speaker are external processes (PipeWire, Pulse, ALSA,
//! sox, ffmpeg — first one found), so this works on Linux and macOS with
//! no native audio crate. `ARBOS_VOICE_MIC_CMD` / `ARBOS_VOICE_PLAYER_CMD`
//! replace them (tests feed a file and swallow the output).
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
/// After Stop, how long the final transcript may take to arrive.
const FINAL_WAIT: Duration = Duration::from_millis(2_000);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceCfg {
    pub url: String,
    pub token: Option<String>,
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

/// A snapshot for the UI: the take so far and the state.
#[derive(Debug, Clone, Default)]
pub struct Peek {
    /// Final segments of the take plus the live partial.
    pub text: String,
    pub phase: Option<Phase>,
    /// Microphone loudness 0..1 (RMS of the last chunk).
    pub level: f32,
    /// What the reply audio is saying, when the server tells us.
    pub reply: String,
    pub error: Option<String>,
    /// `session.ready.engine`: `duplex` answers on its own; `pipeline`
    /// (or an older server that says nothing) leaves replies to us.
    pub engine: String,
}

#[derive(Default)]
struct Shared {
    phase: Option<Phase>,
    engine: String,
    finals: Vec<String>,
    partial: String,
    /// Set when the server closed the take (`transcript.final`).
    take_done: bool,
    reply: String,
    level: f32,
    error: Option<String>,
    /// Bytes of reply audio played, for the tests.
    played: u64,
    interrupts: u32,
}

enum Cmd {
    MicStart,
    MicStop,
    Speak(String),
    Interrupt,
    End,
}

struct Session {
    tx: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Mutex<Shared>>,
    cfg: VoiceCfg,
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
        reply: s.reply.clone(),
        error: s.error.clone(),
        engine: s.engine.clone(),
    }
}

/// Whether the connected server answers by itself (`engine: duplex`). A
/// dictated prompt must then not be sent to the kernel too, and the
/// kernel's answer must not be `speak`-ed: the user would hear two replies.
pub fn server_answers() -> bool {
    status().engine == "duplex"
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
    ensure_session(&cfg)?;
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
    ensure_session(&cfg)?;
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

fn ensure_session(cfg: &VoiceCfg) -> Result<()> {
    {
        let mut hold = hold().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(session) = hold.as_ref() {
            let dead = {
                let s = session.shared.lock().unwrap_or_else(|p| p.into_inner());
                s.phase.is_none() || s.phase == Some(Phase::Off)
            };
            if session.cfg == *cfg && !dead {
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
        crate::agent::acp::runtime().spawn(async move {
            if let Err(e) = run(cfg, rx, Arc::clone(&shared), ready_tx).await {
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
    sink.send(text_frame(json!({
        "type": "session.start",
        "format": { "type": "audio/pcm", "rate": RATE },
        // Replies are ours to drive (`speak`) when the engine leaves them
        // to the client; the agent mirror is off — this window has the chat.
        "reply": "none",
        "agents": false
    })))
    .await?;

    let (mic_tx, mut mic_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let mut mic: Option<Mic> = None;
    let mut player: Option<Player> = None;
    let mut ready_sent = false;
    let mut speaking = false;

    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    Cmd::MicStart => {
                        if mic.is_none() {
                            match Mic::spawn(mic_tx.clone(), Arc::clone(&shared)) {
                                Ok(m) => mic = Some(m),
                                Err(e) => {
                                    let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                                    s.error = Some(format!("microphone: {e:#}"));
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
                    Cmd::Interrupt => {
                        if let Some(p) = player.take() {
                            p.stop();
                        }
                        speaking = false;
                        sink.send(text_frame(json!({ "type": "interrupt" }))).await?;
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
                            match Player::spawn() {
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
                        {
                        let mut s = shared.lock().unwrap_or_else(|p| p.into_inner());
                        match kind {
                            "session.ready" => {
                                s.engine = field("engine");
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
                            }
                            // Increments with their own spacing: append raw.
                            "response.transcript" => s.reply.push_str(&field("text")),
                            "response.done" => {
                                speaking = false;
                                if let Some(p) = player.take() { p.finish(); }
                                if s.phase == Some(Phase::Speaking) {
                                    s.phase = Some(if mic.is_some() { Phase::Listening } else { Phase::Ready });
                                }
                            }
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

/// The microphone: a process writing raw PCM16 mono 24 kHz to stdout, read
/// on a thread in 100 ms chunks.
struct Mic {
    child: Child,
}

impl Mic {
    fn spawn(tx: mpsc::UnboundedSender<Vec<u8>>, shared: Arc<Mutex<Shared>>) -> Result<Self> {
        let mut cmd = mic_command()?;
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("start {}: {e}", cmd.get_program().to_string_lossy()))?;
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
            })
            .map_err(|e| anyhow!("mic thread: {e}"))?;
        Ok(Self { child })
    }

    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The speaker: a process reading raw PCM16 mono 24 kHz from stdin.
struct Player {
    child: Child,
}

impl Player {
    fn spawn() -> Result<Self> {
        let mut cmd = player_command()?;
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow!("start {}: {e}", cmd.get_program().to_string_lossy()))?;
        Ok(Self { child })
    }

    fn write(&mut self, pcm: &[u8]) -> std::io::Result<()> {
        match self.child.stdin.as_mut() {
            Some(stdin) => stdin.write_all(pcm),
            None => Err(std::io::Error::other("player stdin closed")),
        }
    }

    /// The reply is complete: let the player drain and exit on its own.
    fn finish(mut self) {
        drop(self.child.stdin.take());
        std::thread::spawn(move || {
            let _ = self.child.wait();
        });
    }

    /// Barge-in: stop the sound now.
    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(program))
                .any(|f| f.is_file())
        })
        .unwrap_or(false)
}

fn sh(cmd: &str) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd);
    c
}

/// The first microphone program found. `ARBOS_VOICE_MIC_CMD` (a shell
/// command writing raw PCM16 24 kHz mono to stdout) wins.
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
    if cfg!(target_os = "macos") && which("ffmpeg") {
        let mut c = Command::new("ffmpeg");
        c.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "avfoundation",
            "-i",
            ":0",
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
