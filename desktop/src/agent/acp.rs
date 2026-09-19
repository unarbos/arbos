//! One Arbos session over the kernel's attach JSONL seam.
//!
//! The type names stay ACP-shaped so the Arbos transcript and composer keep
//! compiling. The transport is a loopback TCP socket on `arbos-kernel`.

use crate::{
    kernel,
    model::{
        attachment::Prompt,
        place::Place,
        session::{Artifact, ArtifactKind},
    },
};
use anyhow::{Result, anyhow};
pub use arbos_core::wire::Surface as KernelSurface;

use arbos_core::wire::Frame;
use cacp::{
    Error,
    schema::{
        ContentBlock, Cost, Diff, SessionUpdate, StopReason, TextContent, ToolCall,
        ToolCallContent, ToolCallStatus, ToolKind, UsageUpdate,
    },
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    runtime::Runtime,
    sync::mpsc,
};

pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to start the tokio runtime"))
}

pub enum Event {
    Update(SessionUpdate),
    TurnDone(Result<StopReason, Error>),
    Closed,
    /// A message that arrived from outside this window: another chat, or
    /// another door on the same chat.
    Incoming {
        who: String,
        text: String,
    },
    /// The newest transcript line the kernel replayed at attach
    /// (`history_end`): every recorded line at or below it is history, not
    /// news, whatever the pane's cards say about their own lines.
    RecordEnd(u64),
    /// The events that follow, up to `RecordLineEnd`, are one transcript
    /// line's, at this `seq`. The session drops the lot when it holds that
    /// line already — a tail cursor that started at line one, a replay
    /// reaching a live pane (F-135, F-180).
    RecordLine(u64),
    RecordLineEnd,
    /// A person's words the kernel recorded that this window did not send:
    /// spoken through the voice gateway during a call, or typed on the
    /// phone. The transcript's `user` line, with its time.
    UserLine {
        text: String,
        attachments: Vec<String>,
        ts: i64,
        /// The record's transcript line: what a report anchors on.
        seq: u64,
        /// How the words arrived, from the transcript line: `voice` or `text`
        /// (empty on lines from before the kernel wrote it).
        channel: String,
    },
    /// What the kernel holds, in answer to `surfaces`: every job, shell and
    /// page it has, and nothing it does not. A row the window holds that is
    /// absent here has no process behind it.
    Surfaces(Vec<KernelSurface>),
    /// The agent spoke between turns: a callback fired, or background work
    /// finished. Not a turn, and not a failure.
    Aside(String),
    /// A kernel reminder for the model ("project page not updated"): a
    /// dim aside in the transcript, never a failure or a strip.
    Nudge(String),
    /// An attached image the turn's model could not see was described in
    /// words by `model`. Belongs to the user card that carried the image.
    ImageDescribed {
        path: String,
        model: String,
        text: String,
    },
    /// The kernel refused or failed something this window asked for
    /// (`error` frame): shown as a failed notice, kept on the pane.
    Refused(String),
    /// A whole assistant step as the transcript recorded it (an `event`
    /// with a `seq`). Authoritative: it replaces whatever the deltas of
    /// that step built, so the reply never shows twice.
    AssistantFinal {
        text: String,
        step: u64,
    },
    /// Streamed text of model step `step` (1-based within the turn), from
    /// a kernel that numbers its steps; the settled line of the same step
    /// replaces what these built. `step` 0 never reaches here (the plain
    /// chunk path takes it).
    TextDelta {
        text: String,
        step: u64,
    },
    /// A thought's streamed words, by step, likewise.
    ThoughtDelta {
        text: String,
        step: u64,
    },
    /// The settled thought of step `step` (recorded, with its seconds):
    /// stamps the streamed item of that step; opens one when none streamed.
    ThoughtFinal {
        text: String,
        step: u64,
        secs: Option<u32>,
    },
    /// The kernel's notification for this agent (#293): a reply, a
    /// question, a failure or a notice the user may have missed. Replayed
    /// ones (unseen at attach) come oldest first with `replayed`.
    Notify {
        id: u64,
        ts: i64,
        kind: String,
        title: String,
        body: String,
        replayed: bool,
    },
    /// The user has seen every notification with id ≤ `through`, on any
    /// client; every window drops its badge.
    Seen(u64),
    /// Any frame at all arrived on this socket: the kernel is answering.
    /// Sent ahead of the frame's own events so a quiet turn's liveness
    /// clock restarts on frames that draw nothing (a `listing`, a `tree`).
    Alive,
    /// A recorded `wake`: a turn opens (a prompt, a child's report, a
    /// subscription firing); the model's step numbers start again at 1.
    /// A kind other than `user`/`kickoff` is a segment of its own.
    Woke {
        kind: String,
        text: Option<String>,
        at: Option<i64>,
    },
    /// The model call is alive and has been silent for this many seconds
    /// (`working` frame). Live only.
    Working(u64),
    /// A recorded turn ended at this kernel time (ms): the "Worked 25s" of
    /// a transcript read back is the gap from its prompt's `ts`, not the
    /// seconds the replay took to stream.
    TurnEndedAt(i64),
    /// The agent's own word on what it is doing now — the kernel's `status`
    /// event, one line, replaced by the next. Drawn on the parent's
    /// "1 Working  …" line for a worker.
    Status(String),
    /// The parent's "waiting on <worker> — <the worker's step>" line (kernel
    /// #366, `status` with `source: "waiting"`): someone else's step,
    /// watched. `None` when no worker is live any more.
    Waiting(Option<String>),
    /// The kernel's model provider and whether it holds a key (`provider`
    /// frame). `key: false` is the cue to offer this window's own key.
    Provider {
        provider: String,
        model: String,
        key: bool,
        source: String,
    },
    /// The kernel cut the transcript (`rewound`): how many lines went, and
    /// what project state came back, when files were restored.
    Rewound {
        dropped: u64,
        restored: Option<String>,
        /// Files are still being restored; a second `Rewound` follows.
        pending: bool,
    },
    /// The first frame of a connection: the kernel's protocol (`hello`),
    /// or `None` when the kernel predates the handshake.
    Handshake {
        protocol: Option<u32>,
        /// Which kernel answered, as it described itself. Every field is empty
        /// from a kernel that predates the handshake, which is why this is a
        /// build with nothing in it rather than a build assumed to be ours.
        build: crate::kernel::KernelBuild,
    },
    /// The kernel paused the turn for the ask tool.
    NeedQuestion {
        request_id: String,
        title: String,
        questions: Vec<crate::model::session::AskQuestion>,
    },
    /// The material for an in-app report, answered on this connection only:
    /// the exchange the user pointed at, the log for its span, the earlier
    /// transcript lines asked for, any children's lines, and which build
    /// answered — all redacted of credentials by the kernel. Goes to the
    /// review sheet, which shows it before anything is sent.
    Feedback(Box<crate::feedback::Bundle>),
    /// The kernel will not answer a `feedback` ask, in its own words — most
    /// often because it predates the frame. A kernel older than the app is
    /// ordinary: the app carries its own binary and Jacob's places may still be
    /// serving last week's. Better than any version guess, since it is the
    /// kernel itself saying it does not know the frame.
    FeedbackUnavailable(String),
    /// Files a tool made for the user: screenshots, screen recordings.
    Artifacts(Vec<crate::model::session::Artifact>),
    /// The agent presented a file (`show`).
    Show {
        path: String,
        title: String,
        kind: String,
    },
    /// A child session the kernel minted (`delegate` / a scheduled run).
    ChildSession {
        call_id: String,
        session: String,
    },
    /// The kernel opened a terminal, browser, or process under this agent.
    /// `path` is the kernel's id for it (`t1`, `b1`, `j3`); `url` is a
    /// page address or a job's log file.
    Open {
        path: String,
        title: String,
        kind: String,
        cwd: Option<String>,
        url: Option<String>,
        /// Who asked for it: `user`, `agent`, or empty from a kernel that
        /// predates the field (unknown — never read as `user`).
        by: String,
    },
    /// The kernel closed one: the shell exited, the job ended, the page
    /// was dropped.
    Hide {
        path: String,
        kind: String,
    },
    /// What one of the place's shells has written, already decoded.
    ///
    /// Not scoped to this chat: the kernel keys a shell by its page (`t1`)
    /// and stamps every one of them with the agent `root`, whoever asked
    /// for it, so a sub-chat's terminal would be lost by an agent test here.
    /// The window sorts out whose it is and reads one chat's copy
    /// ([`crate::model::pty::PtyStreams`]).
    Pty {
        page: String,
        data: Vec<u8>,
    },
    /// The agent's browser page moved or was pictured.
    Browser {
        page: String,
        url: String,
        screenshot: Option<String>,
    },
    /// Try Live: one frame of the screen the agent works on (PNG bytes),
    /// or why none could be taken.
    Screen {
        machine: String,
        png: Vec<u8>,
        mime: String,
        width: u32,
        height: u32,
        error: Option<String>,
    },
    /// New output from one of the agent's detached jobs.
    Job {
        id: String,
        delta: String,
        running: bool,
        exit: Option<i32>,
    },
    /// The agent's plan, whole. Arrives on attach and after every change.
    Plan(Vec<arbos_core::wire::PlanNode>),
    /// A file of the project store the panel draws moved (`changed`
    /// frame for `notes.md`, `docs/project-context.md`, `archived.md`,
    /// `project.toml`): re-read the store. `path` is relative to `.arbos/`.
    StoreChanged(String),
}

pub type Events = mpsc::UnboundedReceiver<Event>;

pub struct Session {
    reader: tokio::task::JoinHandle<()>,
    out: mpsc::UnboundedSender<String>,
    pub session_id: String,
    pub cwd: PathBuf,
    /// The kernel runs on another machine: a path here means nothing
    /// there, so attachments travel as bytes (`put` with `data`).
    remote: bool,
}

#[derive(Default)]
pub struct Launch {
    pub place: Place,
    pub previous: Option<String>,
    pub status: Option<StatusFn>,
}

pub type StatusFn = Box<dyn Fn(&str) + Send + Sync>;

impl Launch {
    pub fn new(place: Place) -> Self {
        Self {
            place,
            ..Default::default()
        }
    }

    fn say(&self, message: &str) {
        if let Some(status) = &self.status {
            status(message);
        }
    }
}

/// A `feedback_bundle` frame as the CLI prints it — one JSON line — turned into
/// the same `Bundle` the frame path produces.
///
/// One parser, not two: the CLI prints the identical frame
/// ([#466](https://github.com/unarbos/arbos/pull/466)), so the desktop drops it
/// in where the attach's own answer would have gone.
pub fn feedback_bundle_from_json(line: &str) -> Option<crate::feedback::Bundle> {
    match serde_json::from_str::<Frame>(line).ok()? {
        Frame::FeedbackBundle {
            agent,
            turn,
            events,
            tail,
            children,
            log,
            place,
            agents,
            kernel,
            note,
            redacted,
            truncated,
            bytes,
        } => Some(crate::feedback::Bundle {
            agent,
            turn,
            events,
            tail,
            children,
            log,
            place,
            agents,
            kernel,
            note,
            redacted,
            truncated,
            bytes,
        }),
        _ => None,
    }
}

impl Session {
    pub async fn spawn(launch: Launch) -> Result<(Self, Events)> {
        launch.say("attaching to arbos");
        let info = tokio::task::spawn_blocking({
            let place = launch.place.clone();
            move || kernel::attach_or_spawn_place(&place)
        })
        .await
        .map_err(|e| anyhow!("kernel attach task panicked: {e}"))??;

        let addr =
            kernel::tcp_addr(&info.url).ok_or_else(|| anyhow!("bad kernel url {}", info.url))?;
        launch.say("opening session");
        let stream = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr))
            .await
            .map_err(|_| anyhow!("tcp {addr}: timed out"))?
            .map_err(|e| anyhow!("tcp {addr}: {e}"))?;
        let (reader, mut writer) = stream.into_split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        runtime().spawn(async move {
            while let Some(line) = out_rx.recv().await {
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if writer.write_all(b"\n").await.is_err() {
                    break;
                }
            }
        });

        let session_id = match launch.previous.clone() {
            Some(id) if !id.is_empty() => id,
            _ => tokio::task::spawn_blocking({
                let place = launch.place.clone();
                move || kernel::mint_chat(&place)
            })
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_else(|| arbos_core::ROOT_ID.to_string()),
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let agent = session_id.clone();
        let reader = runtime().spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            // The first frame tells what kernel this is: `hello` with a
            // protocol number, or — from a build older than that — anything
            // else. The window refuses to drive a kernel it cannot trust.
            let mut first = true;
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(frame) = serde_json::from_str::<Frame>(&line) {
                    if first {
                        first = false;
                        let hand = match &frame {
                            Frame::Hello {
                                protocol,
                                kernel,
                                git_sha,
                                built_at,
                                binary_gone,
                                ..
                            } => Event::Handshake {
                                protocol: Some(*protocol),
                                build: crate::kernel::KernelBuild {
                                    version: kernel.clone(),
                                    git_sha: git_sha.clone(),
                                    built_at: built_at.clone(),
                                    binary_gone: *binary_gone,
                                },
                            },
                            _ => Event::Handshake {
                                protocol: None,
                                build: crate::kernel::KernelBuild::default(),
                            },
                        };
                        if tx.send(hand).is_err() {
                            return;
                        }
                    }
                    if tx.send(Event::Alive).is_err() {
                        return;
                    }
                    for ev in frame_events(&agent, frame) {
                        if tx.send(ev).is_err() {
                            return;
                        }
                    }
                }
            }
        });

        Ok((
            Self {
                reader,
                out,
                session_id,
                remote: launch.place.is_remote(),
                cwd: launch.place.path,
            },
            rx,
        ))
    }

    /// A prompt for the next turn. While a turn runs the kernel holds it
    /// as an inbox file and runs it when the turn ends.
    pub fn prompt(&self, content: &Prompt) -> Result<()> {
        self.user(content, false)
    }

    /// Words for the turn in flight: the kernel takes them at its next
    /// tool boundary. On an idle agent the kernel treats it as a prompt.
    pub fn steer(&self, content: &Prompt) -> Result<()> {
        self.user(content, true)
    }

    fn user(&self, content: &Prompt, steer: bool) -> Result<()> {
        // Every attachment goes over as a path, images included: the kernel's
        // projection loads image files itself and sends them as pixels
        // (arbos-engine `project::image_paths`). Filtering images out here
        // — a leftover from the base64 `parts` wire — made screenshots
        // arrive as text only. Absolute paths, since the agent's cwd is
        // the place, not wherever the file was picked from.
        //
        // On a remote place the file is sent first: a `put` with its bytes
        // lands it under `.arbos/attachments/` on the kernel's machine, and
        // the user frame names that path. The kernel takes frames in order,
        // so the file is there before the words are. A refusal comes back
        // as a `written` frame with `error`, shown in the chat.
        // A local place gets the same treatment for a file outside it: the
        // agent's tools are confined to the place and its `.arbos/`, so a
        // path under ~/Desktop or /tmp was a file it could not read — it
        // said so and answered without it, where Cursor reads the file
        // (cycle 21). Putting the bytes lands the file under
        // `.arbos/attachments/`, inside the fence.
        let outside = |path: &Path| {
            std::path::absolute(path)
                .map(|abs| !abs.starts_with(&self.cwd))
                .unwrap_or(true)
        };
        let attachments = content
            .attachments
            .iter()
            .map(|a| {
                if self.remote || outside(&a.path) {
                    match self.put_attachment(&a.path) {
                        Ok(stored) => return stored,
                        Err(err) => {
                            eprintln!("attachment {}: {err:#}; sending the path", a.path.display())
                        }
                    }
                }
                std::path::absolute(&a.path)
                    .unwrap_or_else(|_| a.path.clone())
                    .display()
                    .to_string()
            })
            .collect();
        self.send_frame(&Frame::User {
            agent: self.session_id.clone(),
            text: content.text.clone(),
            steer,
            attachments,
            channel: content.channel.clone(),
            device: content.device.clone(),
            model: content.model.clone().unwrap_or_default(),
        })
    }

    /// Send a file's bytes ahead of the prompt that names it. Returns the
    /// relative path the kernel will know it by.
    fn put_attachment(&self, path: &Path) -> Result<String> {
        use base64::Engine;
        // No size check here: the kernel holds the file to `PUT_MAX_BYTES`
        // and its refusal comes back as a `written` frame the chat shows,
        // where a silent fallback to a path the kernel cannot read would
        // not. The tray caps the total before this point anyway.
        let bytes = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let stored = format!("attachments/{stamp}-{name}");
        self.send_frame(&Frame::Put {
            path: stored.clone(),
            text: String::new(),
            data: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
            base_hash: None,
        })?;
        Ok(stored)
    }

    /// The user has seen every notification up to `through`: the kernel
    /// records it and tells every other client.
    pub fn seen(&self, through: u64) -> Result<()> {
        self.send_frame(&Frame::Seen { through })
    }

    /// A probe while a turn is quiet: the kernel answers a `list` with a
    /// `listing` at once, whatever the agent is doing, so silence past it
    /// is the wire's, not the model's.
    pub fn probe(&self) -> Result<()> {
        self.send_frame(&Frame::List {
            path: String::new(),
        })
    }

    pub fn cancel(&self) -> Result<(), Error> {
        self.send_frame(&Frame::Stop {
            agent: self.session_id.clone(),
            reason: None,
        })
        .map_err(|e| Error::internal_error().data(e.to_string()))
    }

    /// Cursor's "Setting up environment" turn on a new Project: asks the
    /// kernel for root's one bounded kickoff turn. A no-op there once root
    /// has any turn on record.
    pub fn kickoff(&self) -> Result<()> {
        self.send_frame(&Frame::Kickoff {
            agent: self.session_id.clone(),
        })
    }

    pub fn skip_question(&self, request_id: &str) -> Result<()> {
        self.answer_questions(request_id, &[], "", true)
    }

    pub fn answer_questions(
        &self,
        request_id: &str,
        answers: &[crate::model::session::AskAnswer],
        details: &str,
        skipped: bool,
    ) -> Result<()> {
        let text = if skipped {
            String::new()
        } else if !details.is_empty() {
            details.to_string()
        } else {
            answers
                .iter()
                .flat_map(|a| a.selected_ids.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = request_id;
        self.send_frame(&Frame::Answer {
            agent: self.session_id.clone(),
            text,
            id: (request_id != self.session_id).then(|| request_id.to_string()),
        })
    }

    /// The kernel socket's write half is gone. A send would fail with
    /// "attach writer closed"; callers reconnect instead of showing that.
    pub fn is_closed(&self) -> bool {
        self.out.is_closed()
    }

    fn send_frame(&self, frame: &Frame) -> Result<()> {
        let line = serde_json::to_string(frame)?;
        self.out
            .send(line)
            .map_err(|_| anyhow!("attach writer closed"))
    }

    /// Hand the kernel a provider and key (`configure`); owner only.
    pub fn configure(
        &self,
        provider: &str,
        api_base: &str,
        model: &str,
        api_key: &str,
        remember: bool,
    ) {
        let _ = self.send_frame(&Frame::Configure {
            provider: provider.to_string(),
            api_base: api_base.to_string(),
            model: model.to_string(),
            api_key: api_key.to_string(),
            remember,
        });
    }

    pub fn set_model(&self, model: &str) {
        let _ = self.send_frame(&Frame::SetModel {
            agent: self.session_id.clone(),
            model: model.to_string(),
        });
    }

    pub fn undo_checkpoint(&self) {
        let _ = self.send_frame(&Frame::Undo {
            agent: self.session_id.clone(),
        });
    }

    /// Summarise the oldest turns now (`/compact`).
    pub fn compact(&self) {
        let _ = self.send_frame(&Frame::Compact {
            agent: self.session_id.clone(),
        });
    }

    /// Put the agent back to the start of its `turn`-th user turn
    /// ("Rewind here"); `files` restores the project too.
    pub fn rewind(&self, turn: u32, line: Option<u64>, files: bool) {
        let _ = self.send_frame(&Frame::Rewind {
            agent: self.session_id.clone(),
            turn,
            files,
            line,
        });
    }

    /// Pause or resume the agent (`/pause`, `/resume`).
    pub fn set_paused(&self, paused: bool) {
        let _ = self.send_frame(&Frame::Pause {
            agent: self.session_id.clone(),
            paused,
        });
    }

    /// Try Live: ask for the screen the agent works on. The answer comes
    /// back as `Event::Screen`.
    pub fn request_screen(&self) {
        let _ = self.send_frame(&Frame::Screen {
            agent: self.session_id.clone(),
        });
    }

    /// Ask for the material behind a report: the exchange holding `seq` (a
    /// line the user is looking at) or the last one he opened, `tail` lines
    /// of transcript before it, and `call_id`'s whole output when he pointed
    /// at a particular call. The answer comes back as `Event::Feedback`.
    pub fn request_feedback(
        &self,
        seq: Option<u64>,
        call_id: Option<String>,
        tail: u32,
        note: &str,
    ) {
        let _ = self.send_frame(&Frame::Feedback {
            agent: self.session_id.clone(),
            seq,
            call_id,
            tail,
            note: note.to_string(),
        });
    }

    /// Ask for a shell of this person's own: their `$SHELL`, interactive, in
    /// `cwd`. The kernel answers with a `board` frame carrying `by: user`, so
    /// the row arrives already knowing whose it is and the drawer opens for it
    /// (#461).
    pub fn shell(&self, cwd: Option<String>) {
        let _ = self.send_frame(&Frame::Shell { owner: None, cwd });
    }

    /// Keys, a paste, or the emulator's answer to the shell's own query,
    /// on the shell `page` (`t1`). The kernel keys its shells under the
    /// agent `root` whoever asked for them, so that is the name here.
    pub fn pty_in(&self, page: &str, bytes: &[u8]) {
        use base64::Engine;
        let _ = self.send_frame(&Frame::PtyIn {
            agent: arbos_core::ROOT_ID.to_string(),
            page: page.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }

    /// Ask for a browser page of this person's own. The kernel answers
    /// with a `board` frame carrying `by: user`.
    pub fn browse(&self, url: Option<String>) {
        let _ = self.send_frame(&Frame::Browse { owner: None, url });
    }

    /// Stream the agent's page to this window, or stop.
    pub fn watch_browser(&self, on: bool) {
        let _ = self.send_frame(&Frame::BrowserWatch {
            agent: self.session_id.clone(),
            on,
        });
    }

    /// Hold or release a path the person is editing.
    pub fn claim(&self, path: String, held: bool) {
        let _ = self.send_frame(&Frame::Claim { path, held });
    }

    /// Compare-and-swap save of a project file. The window already wrote
    /// locally; this tells other clients. `base_hash` is the hash after
    /// the write, so a second save from here is a no-op conflict unless
    /// the kernel still has the old bytes — we send the new hash as both
    /// content identity. The kernel's `save` is the other window's path;
    /// this call is best-effort.
    pub fn save_file(&self, path: String, text: String, hash: String) {
        let _ = self.send_frame(&Frame::Save {
            path,
            text,
            base_hash: hash,
        });
    }

    /// Ask the kernel what it holds — its jobs, shells and pages, with their
    /// states. Sent when a connection comes back, because the kernel that
    /// answers may not be the one that opened those rows: a kernel that died
    /// is replaced, and the replacement knows nothing of its shells. The
    /// answer arrives on this connection only.
    pub fn surfaces(&self, agent: Option<String>) {
        let _ = self.send_frame(&Frame::Surfaces { agent });
    }

    /// Move a plan node from the window: `cancel`, `run`, `reopen`, `answer`.
    pub fn plan_op(&self, node: u64, op: &str, text: &str) {
        let _ = self.send_frame(&Frame::PlanOp {
            agent: self.session_id.clone(),
            node,
            op: op.to_string(),
            text: text.to_string(),
        });
    }

    /// The kernel's permission modes: auto, ask, plan.
    pub fn set_mode(&self, mode_id: &str) {
        let _ = self.send_frame(&Frame::SetMode {
            agent: self.session_id.clone(),
            mode: mode_id.to_string(),
        });
    }

    /// The Mode switch the composer draws for a kernel chat: the three
    /// permission modes, `current` from the agent's folder.
    pub fn modes(current: &str) -> cacp::schema::SessionModeState {
        use cacp::schema::{SessionMode, SessionModeId, SessionModeState};
        let current = arbos_core::Mode::parse(current).unwrap_or_default();
        SessionModeState {
            current_mode_id: SessionModeId::from(current.as_str()),
            available_modes: arbos_core::Mode::ALL
                .iter()
                .map(|m| SessionMode {
                    id: SessionModeId::from(m.as_str()),
                    name: m.label().to_string(),
                    description: Some(m.describe().to_string()),
                    meta: None,
                })
                .collect(),
            meta: None,
        }
    }

    /// No-op. The kernel has no ACP config options; the model is `set_model`.
    pub fn set_config_option(
        &self,
        _config_id: &str,
        _value: cacp::schema::SessionConfigOptionValue,
    ) {
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

fn frame_events(agent: &str, frame: Frame) -> Vec<Event> {
    match frame {
        Frame::Event { agent: id, event } if id == agent || agent.is_empty() => {
            let seq = event.seq;
            let mut out = kernel_event(&id, event);
            if seq > 0 {
                out.insert(0, Event::RecordLine(seq));
                out.push(Event::RecordLineEnd);
            }
            out
        }
        // Streamed text, one chunk per frame (the kernel's live path); the
        // whole step arrives later as an `event` with a seq, which
        // `merge_stream_text` folds into what the chunks built.
        Frame::AssistantDelta { agent: id, text, step } if id == agent || agent.is_empty() => {
            if step > 0 {
                vec![Event::TextDelta { text, step }]
            } else {
                vec![Event::Update(SessionUpdate::AgentMessageChunk(text_chunk(
                    text,
                )))]
            }
        }
        // Not filtered by agent: the list is the place's, and the connection
        // it arrives on is the one that asked.
        Frame::SurfaceList { surfaces, .. } => vec![Event::Surfaces(surfaces)],
        Frame::Working { agent: id, secs } if id == agent || agent.is_empty() => {
            vec![Event::Working(secs)]
        }
        Frame::Notify {
            id: nid,
            ts,
            agent: who,
            kind,
            title,
            body,
            replayed,
        } if who == agent => vec![Event::Notify {
            id: nid,
            ts,
            kind,
            title,
            body,
            replayed,
        }],
        Frame::Seen { through } => vec![Event::Seen(through)],
        Frame::Provider {
            provider,
            model,
            key,
            source,
        } => vec![Event::Provider {
            provider,
            model,
            key,
            source,
        }],
        Frame::ThinkingDelta { agent: id, text, step } if id == agent || agent.is_empty() => {
            if step > 0 {
                vec![Event::ThoughtDelta { text, step }]
            } else {
                vec![Event::Update(SessionUpdate::AgentThoughtChunk(text_chunk(
                    text,
                )))]
            }
        }
        Frame::Turn {
            agent: id,
            state,
            budget,
        } if id == agent => {
            let mut out = Vec::new();
            if let Some(b) = budget {
                out.push(Event::Update(SessionUpdate::UsageUpdate(UsageUpdate {
                    used: b.used,
                    size: b.size,
                    cost: None,
                    meta: None,
                })));
            }
            if state == "idle" {
                out.push(Event::TurnDone(Ok(StopReason::EndTurn)));
            }
            out
        }
        Frame::Ask {
            agent: id,
            question,
            options,
            id: ask_id,
        } if id == agent => vec![Event::NeedQuestion {
            // The kernel's ask id when it sends one (qa-021): the answer
            // echoes it, so a late or duplicate answer cannot resolve a
            // different question. Older kernels: the agent id, sent blind.
            request_id: ask_id.unwrap_or(id),
            title: question.clone(),
            questions: vec![crate::model::session::AskQuestion {
                id: "q".into(),
                prompt: question,
                options: options
                    .into_iter()
                    .map(|label| crate::model::session::AskOption {
                        id: label.clone(),
                        label,
                    })
                    .collect(),
                allow_multiple: false,
            }],
        }],
        // The replay's close names the newest line on the record at attach:
        // the pane's high-water mark for "already held" starts there.
        Frame::HistoryEnd { agent: id, to, .. } if id == agent => vec![Event::RecordEnd(to)],
        // The desktop reads transcripts from the files; the replay that a
        // file-less client needs is not for it. Skipped here so a replayed
        // line is never appended a second time.
        Frame::Snapshot { .. }
        | Frame::Tree { .. }
        | Frame::Hello { .. }
        | Frame::Configure { .. }
        | Frame::Replayed { .. }
        | Frame::HistoryEnd { .. }
        // Client → kernel, so it never arrives here. `feedback_bundle` does,
        // and is taken below.
        | Frame::Feedback { .. }
        | Frame::ToolBody { .. }
        | Frame::ToolBodyReply { .. }
        // A newer kernel's frame: nothing to show, nothing to lose.
        | Frame::Unknown => Vec::new(),
        Frame::Rewound {
            agent: id,
            dropped,
            restored,
            pending,
            ..
        } if id == agent => vec![Event::Rewound {
            dropped,
            restored,
            pending,
        }],
        // The kernel's answer to a bad ask (an unknown agent, a rewind it
        // cannot do): the reason belongs in the chat, not in a log.
        Frame::Error {
            agent: Some(id),
            detail,
        } if id == agent => vec![Event::Refused(detail)],
        // An unknown frame is refused with no agent on it, so this used to fall
        // off the end of the match and be dropped — which is why a sheet on an
        // old kernel sat reading "still reading the exchange" for ever instead
        // of saying what was wrong.
        Frame::Error {
            agent: None,
            detail,
        } if detail.contains("unknown frame type") && detail.contains("feedback") => {
            vec![Event::FeedbackUnavailable(detail)]
        }
        // A `put` of an attachment's bytes the kernel would not take (too
        // large, a bad path): the words went through without the file, and
        // the chat says so.
        Frame::Written {
            path,
            error: Some(error),
            ..
        } if path.starts_with("attachments/") => {
            let name = path.rsplit('/').next().unwrap_or(&path);
            let name = name.split_once('-').map_or(name, |(_, rest)| rest);
            vec![Event::Refused(format!("attachment {name} not sent: {error}"))]
        }
        Frame::Plan { agent: id, nodes } if id == agent => vec![Event::Plan(nodes)],
        // The agent's own line on what it is doing (or the kernel's guess
        // from the tool in flight); an empty step means idle.
        Frame::Status {
            agent: id,
            step,
            source,
            ..
        } if id == agent && source == "waiting" => {
            let step = step.trim().to_string();
            vec![Event::Waiting((!step.is_empty()).then_some(step))]
        }
        Frame::Status { agent: id, step, .. } if id == agent => vec![Event::Status(step)],
        // Not agent-scoped: every attached chat hears it, and the
        // workspace's re-read is idempotent.
        Frame::Changed { path, .. } if store_file(&path) => vec![Event::StoreChanged(path)],
        // A shell's output. Decoded here, on the reader task, rather than on
        // the window's own thread: this is the one frame that arrives for
        // every keystroke a person makes.
        Frame::Pty { page, data, .. } => {
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(data.as_bytes()) {
                Ok(bytes) => vec![Event::Pty { page, data: bytes }],
                Err(_) => Vec::new(),
            }
        }
        Frame::Board {
            owner,
            action,
            panel,
            terminal_ids,
            cwd,
            title,
            url,
            by,
        } if owner == agent && matches!(panel.as_str(), "terminal" | "browser" | "process") => {
            let title = title.unwrap_or_default();
            terminal_ids
                .into_iter()
                .map(|id| {
                    if action == "close" {
                        Event::Hide {
                            path: id,
                            kind: panel.clone(),
                        }
                    } else {
                        Event::Open {
                            path: id,
                            title: title.clone(),
                            kind: panel.clone(),
                            cwd: cwd.clone(),
                            url: url.clone(),
                            by: by.clone(),
                        }
                    }
                })
                .collect()
        }
        Frame::Browser {
            agent: id,
            page,
            url,
            screenshot,
        } if id == agent => vec![Event::Browser {
            page,
            url,
            screenshot,
        }],
        Frame::BrowserFrame {
            agent: id,
            page,
            data,
            ..
        } if id == agent => vec![Event::Browser {
            page,
            url: String::new(),
            screenshot: Some(data),
        }],
        Frame::FeedbackBundle {
            agent: id,
            turn,
            events,
            tail,
            children,
            log,
            kernel,
            place,
            agents,
            note,
            redacted,
            truncated,
            bytes,
        } if id == agent => vec![Event::Feedback(Box::new(crate::feedback::Bundle {
            agent: id,
            turn,
            events,
            tail,
            children,
            log,
            kernel,
            place,
            agents,
            note,
            redacted,
            truncated,
            bytes,
        }))],
        Frame::Screenshot {
            agent: id,
            machine,
            png,
            mime,
            width,
            height,
            error,
            ..
        } if id == agent => {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(png.as_bytes())
                .unwrap_or_default();
            vec![Event::Screen {
                machine,
                png: bytes,
                mime,
                width,
                height,
                error,
            }]
        }
        Frame::Job {
            agent: id,
            id: job,
            delta,
            running,
            exit,
        } if id == agent => vec![Event::Job {
            id: job,
            delta,
            running,
            exit,
        }],
        _ => Vec::new(),
    }
}

/// The store files the right panel reads: the project page, the context
/// document, the archive, and the project's face.
fn store_file(path: &str) -> bool {
    matches!(
        path,
        "notes.md" | "archived.md" | "docs/project-context.md" | "project.toml" | "GOALS.md"
    )
}

fn kernel_event(agent: &str, event: arbos_core::Event) -> Vec<Event> {
    use arbos_core::EventKind;
    let recorded = event.seq > 0;
    let ts = event.ts;
    match event.kind {
        // Another client's prompt landed on the record. This window's own
        // prompts are on the pane already; the session tells them apart.
        EventKind::User {
            text,
            attachments,
            channel,
            ..
        } if recorded => vec![Event::UserLine {
            text,
            attachments,
            ts,
            seq: event.seq,
            channel,
        }],
        // The kickoff turn opening live: its step reads as Cursor's
        // "Setting up environment" until the agent names one of its own.
        EventKind::Wake { wake, .. } if wake == "kickoff" && !recorded => {
            vec![Event::Status("Setting up environment".into())]
        }
        EventKind::Wake { wake, text, .. } if recorded => vec![Event::Woke {
            kind: wake,
            text,
            at: (ts > 0).then_some(ts),
        }],
        // A transcript line (tailed or replayed) is the step's final text;
        // a live emit without a seq is a delta (older kernels send those
        // as events too).
        EventKind::Assistant { text, step, .. } if recorded => {
            vec![Event::AssistantFinal { text, step }]
        }
        EventKind::Assistant { text, .. } => {
            vec![Event::Update(SessionUpdate::AgentMessageChunk(text_chunk(
                text,
            )))]
        }
        // A settled thinking record (recorded, with `secs`) follows the
        // deltas that already built the thought: nothing to add live. A
        // recorded thought without `secs` is an ACP worker's only form of
        // it and still shows.
        EventKind::Thinking {
            text,
            secs: Some(secs),
            step,
        } if recorded && step > 0 => vec![Event::ThoughtFinal {
            text,
            step,
            secs: Some(secs.min(u32::MAX as u64) as u32),
        }],
        EventKind::Thinking { secs: Some(_), .. } if recorded => Vec::new(),
        EventKind::Thinking { text, .. } => {
            vec![Event::Update(SessionUpdate::AgentThoughtChunk(text_chunk(
                text,
            )))]
        }
        EventKind::Say { from, text } => vec![Event::Incoming { who: from, text }],
        // A keyless kernel kept the typed line for a key (#312): no turn
        // was spent, so this is not a turn failure — the same words as its
        // `error` frame, which the session reads once.
        EventKind::Notice { text, failed: true }
            if text.contains(crate::model::session::LINE_KEPT_FOR_KEY) =>
        {
            vec![Event::Refused(text)]
        }
        EventKind::Notice { text, failed: true } => {
            vec![Event::TurnDone(Err(Error::internal_error().data(text)))]
        }
        EventKind::Notice { text, .. } => vec![Event::Aside(text)],
        EventKind::ImageDescribed { path, model, text } => {
            vec![Event::ImageDescribed { path, model, text }]
        }
        EventKind::Nudge { text, .. } => vec![Event::Nudge(text)],
        // The turn was cut short; the pane says by whom (the fold line
        // picks the same text up).
        EventKind::Interrupted { detail } => vec![Event::Aside(
            crate::model::session::interrupt_label(&detail),
        )],
        EventKind::Ask {
            question,
            options,
            call_id,
        } => vec![Event::NeedQuestion {
            // The transcript line carries the ask's id when the kernel
            // wrote one; without it the answer goes blind (the agent id),
            // which the kernel accepts while one question is pending. Never
            // a made-up id: the kernel refuses those (ui-004).
            request_id: call_id.unwrap_or_else(|| agent.to_string()),
            title: question.clone(),
            questions: vec![crate::model::session::AskQuestion {
                id: "q".into(),
                prompt: question,
                options: options
                    .into_iter()
                    .map(|label| crate::model::session::AskOption {
                        id: label.clone(),
                        label,
                    })
                    .collect(),
                allow_multiple: false,
            }],
        }],
        EventKind::Tool(rec) => {
            let mut tool = ToolCall::new(rec.call_id.as_str(), rec.name.as_str());
            tool.kind = tool_kind(&rec.name);
            let hint = tool_hint(&rec.name, &rec.paths, rec.args.as_ref());
            tool.title = tool_title(&rec.name, hint.as_deref());
            if let Some(label) = rec
                .label
                .as_deref()
                .map(str::trim)
                .filter(|l| !l.is_empty())
            {
                let mut meta = serde_json::Map::new();
                meta.insert("label".into(), serde_json::Value::String(label.to_string()));
                tool.meta = Some(meta);
            }
            // The kernel emits the call once as it starts — no `ended`, no
            // body — and again when it returns. The first is a running
            // card, not a finished one: read as complete it drew a green
            // "Ran …" with nothing in it for the whole of a 34 s command
            // (F-220, cycle 58). A recorded line always has its end.
            tool.status = if rec.error.is_some() {
                ToolCallStatus::Failed
            } else if rec.ended.is_none() && !recorded {
                ToolCallStatus::InProgress
            } else {
                ToolCallStatus::Completed
            };
            let body_text = rec.body.clone();
            if let Some(body) = rec.body {
                tool.content.push(ToolCallContent::Content {
                    content: ContentBlock::Text(TextContent {
                        text: body,
                        annotations: None,
                        meta: None,
                    }),
                });
            }
            if let Some(diff) = display_diff(&rec.name, rec.args.as_ref(), rec.diff.as_deref()) {
                tool.content.push(ToolCallContent::Diff(Diff {
                    path: PathBuf::new(),
                    new_text: diff,
                    old_text: None,
                    meta: None,
                }));
            }
            let mut out = vec![Event::Update(SessionUpdate::ToolCall(tool))];
            let files = artifacts(&rec.name, &rec.paths, &rec.images, body_text.as_deref());
            if !files.is_empty() {
                out.push(Event::Artifacts(files));
            }
            // `spawn` names the agent it minted. The row goes under the
            // parent now, not on the next activity poll.
            if let Some(child) = rec.child.filter(|id| !id.is_empty()) {
                out.push(Event::ChildSession {
                    call_id: rec.call_id,
                    session: child,
                });
            }
            out
        }
        // Usage only. The end of the turn is `Frame::Turn idle`, sent once
        // per job by the scheduler; the tailed `TurnComplete` arrives up to
        // 200 ms later and a second TurnDone would drain a queued follow-up
        // into a turn that is already running.
        EventKind::TurnComplete { usage, .. } if recorded => usage
            .map(|u| {
                vec![Event::Update(SessionUpdate::UsageUpdate(UsageUpdate {
                    used: u.used,
                    size: u.size,
                    cost: u.cost.map(|amount| Cost {
                        amount,
                        currency: "USD".into(),
                        meta: None,
                    }),
                    meta: None,
                }))]
            })
            .unwrap_or_default()
            .into_iter()
            .chain(std::iter::once(Event::TurnEndedAt(ts)))
            .collect(),
        EventKind::TurnComplete { usage, .. } => usage
            .map(|u| {
                vec![Event::Update(SessionUpdate::UsageUpdate(UsageUpdate {
                    used: u.used,
                    size: u.size,
                    cost: u.cost.map(|amount| Cost {
                        amount,
                        currency: "USD".into(),
                        meta: None,
                    }),
                    meta: None,
                }))]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn _legacy_ws_removed() {}

/// The files a tool call made for the user. Pictures the call produced
/// (`images`) each get a card; a clip listed in `paths` gets one card with
/// the call's picture as its poster. `read` on an image is looking, not
/// making, so it adds nothing here.
pub(crate) fn artifacts(
    name: &str,
    paths: &[String],
    images: &[String],
    body: Option<&str>,
) -> Vec<Artifact> {
    if !matches!(name, "screenshot" | "record" | "browser") {
        return Vec::new();
    }
    let caption = body.map(artifact_caption).unwrap_or_default();
    let clips: Vec<&String> = paths
        .iter()
        .filter(|p| ArtifactKind::of_path(p) == ArtifactKind::Video)
        .collect();
    if !clips.is_empty() {
        return clips
            .into_iter()
            .map(|clip| Artifact::load(clip, images.first().map(String::as_str), &caption))
            .collect();
    }
    images
        .iter()
        .map(|image| Artifact::load(image, None, &caption))
        .collect()
}

/// `(10.2s, 177 KB, via ffmpeg x11grab)` → `10.2s · 177 KB`. The tools
/// put their measurements in the first parenthesis; keep the sizes.
fn artifact_caption(body: &str) -> String {
    let Some(start) = body.find('(') else {
        return String::new();
    };
    let Some(len) = body[start..].find(')') else {
        return String::new();
    };
    body[start + 1..start + len]
        .split(',')
        .map(str::trim)
        .filter(|part| {
            !part.is_empty()
                && !part.starts_with("via ")
                && !part.starts_with("image/")
                && part.chars().next().is_some_and(|c| c.is_ascii_digit())
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// ChatView DiffCard: a write is every new line as an add. Prefer the
/// kernel's stored `diff` when it exists (edits). Writes rarely have one.
pub(crate) fn display_diff(
    name: &str,
    args: Option<&Value>,
    stored: Option<&str>,
) -> Option<String> {
    if let Some(diff) = stored
        .map(str::trim)
        .filter(|diff| !diff.is_empty() && !diff.starts_with("(diff omitted"))
    {
        return Some(diff.to_owned());
    }
    if name != "write" {
        return None;
    }
    let parsed = args.and_then(|args| match args {
        Value::String(raw) => serde_json::from_str::<Value>(raw).ok(),
        other => Some(other.clone()),
    })?;
    let body = parsed
        .get("contents")
        .or_else(|| parsed.get("content"))
        .and_then(Value::as_str)
        .filter(|body| !body.is_empty())?;
    Some(
        body.lines()
            .enumerate()
            .map(|(i, line)| format!("+{} {line}", i + 1))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// The kernel's line-numbered display diff. The model never sees it.
pub(crate) fn result_diff(result: &Value) -> Option<String> {
    result
        .get("Details")
        .or_else(|| result.get("details"))
        .and_then(|details| details.get("diff").or_else(|| details.get("Diff")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|diff| !diff.is_empty() && !diff.starts_with("(diff omitted"))
        .map(str::to_owned)
}

fn json_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub(crate) fn child_session(details: &Value) -> Option<String> {
    json_str(details, &["childSession", "child_session"]).map(str::to_owned)
}

pub(crate) fn tool_kind(name: &str) -> ToolKind {
    match name {
        "read" | "show" => ToolKind::Read,
        "write" | "edit" | "apply_patch" => ToolKind::Edit,
        "bash" | "terminal" => ToolKind::Execute,
        "grep" | "find" | "ls" | "tgrep" | "glob" => ToolKind::Search,
        "web" | "fetch" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

/// Title the transcript shows: the verb, plus a file or query when we have one.
pub(crate) fn tool_title(name: &str, hint: Option<&str>) -> String {
    let name = name.trim();
    let Some(hint) = hint.map(str::trim).filter(|hint| !hint.is_empty()) else {
        return name.to_owned();
    };
    let keep_full = matches!(
        name,
        "bash"
            | "run"
            | "exec"
            | "grep"
            | "find"
            | "tgrep"
            | "glob"
            | "plan"
            | "subscribe"
            | "say"
            | "ask"
            | "spawn"
            | "remember"
    );
    let shown = if keep_full {
        hint
    } else {
        hint.rsplit(['/', '\\']).next().unwrap_or(hint)
    };
    if name.is_empty() {
        return shown.to_owned();
    }
    format!("{name} {shown}")
}

/// Path, pattern, or command from a kernel tool-call payload.
/// Shell tools must use `command`, not the job log in `paths` (`out.log`).
pub(crate) fn tool_path_hint(value: &Value) -> Option<String> {
    let name = json_str(value, &["Name", "name"]).unwrap_or("");
    let paths: Vec<String> = value
        .get("paths")
        .or_else(|| value.get("Paths"))
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let args = value.get("Args").or_else(|| value.get("args"));
    tool_hint(name, &paths, args)
}

pub(crate) fn tool_hint(name: &str, paths: &[String], args: Option<&Value>) -> Option<String> {
    let parsed = args.and_then(|args| match args {
        Value::String(raw) => serde_json::from_str::<Value>(raw).ok(),
        other => Some(other.clone()),
    });
    let obj = parsed.as_ref().filter(|value| value.is_object());
    // The kernel's own tools: say what was asked, not a path.
    if let Some(summary) = obj.and_then(|o| kernel_tool_hint(name, o)) {
        return Some(summary);
    }
    let command = obj
        .and_then(|obj| obj.get("command").and_then(Value::as_str))
        .map(clean_shell)
        .filter(|cmd| !cmd.is_empty());
    if matches!(name, "bash" | "run" | "exec" | "terminal") && command.is_some() {
        return command;
    }
    if let Some(path) = paths
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .find(|path| !path.is_empty() && !is_job_log(path))
    {
        return Some(path.to_owned());
    }
    // A page fetched is named by its host, as Cursor's "Fetched
    // en.wikipedia.org" — a bare "fetch" / "Fetched fetch" was what Jacob
    // saw on every web page his agent read (report 2026-09-17-12, F-143).
    if matches!(name, "fetch" | "web")
        && let Some(url) = obj.and_then(|obj| obj.get("url").and_then(Value::as_str))
    {
        let host = url
            .split("://")
            .nth(1)
            .unwrap_or(url)
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(url)
            .trim_start_matches("www.");
        if !host.is_empty() {
            return Some(host.to_owned());
        }
    }
    obj.and_then(|obj| {
        [
            "path", "file", "target", "pattern", "query", "command", "url",
        ]
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(str::to_owned)
    })
}

/// `plan set · 12 items`, `plan check 3`, `subscribe add timer every 1h`,
/// `say to=root`, `ask "which name…"`, `spawn "brief…"`.
fn kernel_tool_hint(name: &str, o: &Value) -> Option<String> {
    let s = |k: &str| {
        o.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let count = |k: &str| o.get(k).and_then(Value::as_array).map(|a| a.len());
    let head = |t: &str| {
        let line = t.lines().next().unwrap_or("");
        let cut: String = line.chars().take(40).collect();
        if cut.chars().count() < line.chars().count() {
            format!("{cut}…")
        } else {
            cut
        }
    };
    match name {
        "plan" => {
            let op = s("op")
                .or_else(|| s("action"))
                .unwrap_or(if o.get("items").is_some() { "set" } else { "" });
            let n = ["items", "goals", "nodes", "steps", "tasks"]
                .iter()
                .find_map(|k| count(k));
            let mut out = op.to_string();
            if let Some(n) = n {
                out.push_str(&format!(" · {n} item{}", if n == 1 { "" } else { "s" }));
            } else if let Some(k) = o.get("n").and_then(Value::as_u64) {
                out.push_str(&format!(" {k}"));
            } else if let Some(t) = s("text") {
                out.push_str(&format!(" \"{}\"", head(t)));
            }
            Some(out.trim().to_string()).filter(|v| !v.is_empty())
        }
        "subscribe" => {
            let mut out = s("op").unwrap_or("add").to_string();
            if let Some(k) = s("kind") {
                out.push(' ');
                out.push_str(k);
            }
            if let Some(e) = s("every") {
                out.push_str(&format!(" every {e}"));
            } else if let Some(a) = s("after") {
                out.push_str(&format!(" after {a}"));
            }
            if let Some(id) = o.get("id").and_then(Value::as_u64) {
                out.push_str(&format!(" #{id}"));
            }
            Some(out)
        }
        "say" => s("to").map(|to| format!("to={to}")),
        "ask" => s("question").map(|q| format!("\"{}\"", head(q))),
        "spawn" => s("brief").map(|b| format!("\"{}\"", head(b))),
        "remember" => s("fact")
            .or_else(|| s("text"))
            .map(|f| format!("\"{}\"", head(f))),
        _ => None,
    }
}

fn is_job_log(path: &str) -> bool {
    let leaf = path.rsplit(['/', '\\']).next().unwrap_or(path);
    leaf == "out.log" || leaf == "err.log" || path.contains("/jobs/")
}

fn clean_shell(cmd: &str) -> String {
    let mut s = cmd.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s = s[1..s.len() - 1].trim();
    }
    s.trim_start_matches('|').trim().to_owned()
}

fn text_chunk(text: String) -> cacp::schema::ContentChunk {
    cacp::schema::ContentChunk {
        content: ContentBlock::Text(TextContent {
            text,
            annotations: None,
            meta: None,
        }),
        message_id: None,
        meta: None,
    }
}

pub fn error_text(e: &Error) -> String {
    match &e.data {
        Some(data) => {
            let detail = data
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| data.to_string());
            format!("{} — {detail}", e.message)
        }
        None => e.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::attachment::{Attachment, Prompt};

    fn session() -> (Session, mpsc::UnboundedReceiver<String>) {
        let (out, rx) = mpsc::unbounded_channel();
        let session = Session {
            reader: runtime().spawn(async {}),
            out,
            session_id: "s1".into(),
            cwd: PathBuf::new(),
            remote: false,
        };
        (session, rx)
    }

    fn png_file(name: &str) -> PathBuf {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let path =
            std::env::temp_dir().join(format!("arbos-acp-{}-{name}.png", std::process::id()));
        std::fs::write(&path, bytes.into_inner()).unwrap();
        path
    }

    /// The kernel loads image attachments from disk and projects them as
    /// pixels, so an image must go over as its absolute path — dropping it
    /// is what made a pasted screenshot arrive as text only.
    #[test]
    fn image_attachment_is_sent_as_an_absolute_path() {
        let path = png_file("attach");
        let attachment = Attachment::load(path.clone()).unwrap();
        assert!(attachment.is_image());
        let (session, mut rx) = session();
        session
            .prompt(&Prompt::compose("what is this?", vec![attachment]))
            .unwrap();
        let frame: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(frame["type"], "user");
        assert_eq!(frame["agent"], "s1");
        assert_eq!(frame["text"], "what is this?");
        let sent = frame["attachments"].as_array().unwrap();
        assert_eq!(sent.len(), 1);
        let sent = PathBuf::from(sent[0].as_str().unwrap());
        assert!(sent.is_absolute());
        assert_eq!(sent, std::path::absolute(&path).unwrap());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn text_only_prompt_omits_attachments() {
        let (session, mut rx) = session();
        session.prompt(&Prompt::compose("hello", vec![])).unwrap();
        let frame: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(frame["text"], "hello");
        assert!(frame.get("attachments").is_none());
    }
}
