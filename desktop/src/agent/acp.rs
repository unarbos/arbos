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
use arbos_core::wire::Frame;
use cacp::{
    Error,
    schema::{
        ContentBlock, Cost, Diff, RequestPermissionRequest, RequestPermissionResponse,
        SessionUpdate, StopReason, TextContent, ToolCall, ToolCallContent, ToolCallStatus,
        ToolKind, UsageUpdate,
    },
};
use serde_json::Value;
use std::{path::PathBuf, sync::OnceLock, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    runtime::Runtime,
    sync::{mpsc, oneshot},
};

pub fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to start the tokio runtime"))
}

pub enum Event {
    History(crate::model::history::Replay),
    Update(SessionUpdate),
    Permission(RequestPermissionRequest, Reply<RequestPermissionResponse>),
    TurnDone(Result<StopReason, Error>),
    Reconnecting,
    Reconnected,
    Closed,
    /// A message that arrived from outside this window: another chat, or
    /// another door on the same chat.
    Incoming {
        who: String,
        text: String,
    },
    /// The agent spoke between turns: a callback fired, or background work
    /// finished. Not a turn, and not a failure.
    Aside(String),
    /// The kernel refused or failed something this window asked for
    /// (`error` frame): shown as a failed notice, kept on the pane.
    Refused(String),
    /// A whole assistant step as the transcript recorded it (an `event`
    /// with a `seq`). Authoritative: it replaces whatever the deltas of
    /// that step built, so the reply never shows twice.
    AssistantFinal(String),
    /// The model call is alive and has been silent for this many seconds
    /// (`working` frame). Live only.
    Working(u64),
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
    },
    /// The first frame of a connection: the kernel's protocol (`hello`),
    /// or `None` when the kernel predates the handshake.
    Handshake {
        protocol: Option<u32>,
        kernel: String,
    },
    /// The kernel paused the turn for a tool the user must allow.
    NeedApproval {
        request_id: String,
        title: String,
    },
    /// The kernel paused the turn for the ask tool.
    NeedQuestion {
        request_id: String,
        title: String,
        questions: Vec<crate::model::session::AskQuestion>,
    },
    /// Provider-generated pictures for the turn that just finished.
    Images(Vec<crate::model::attachment::MessageImage>),
    /// Files a tool made for the user: screenshots, screen recordings.
    Artifacts(Vec<crate::model::session::Artifact>),
    /// Web-search sources the provider grounded the last assistant message on.
    Citations(Vec<Citation>),
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
    },
    /// The kernel closed one: the shell exited, the job ended, the page
    /// was dropped.
    Hide {
        path: String,
        kind: String,
    },
    /// The agent's browser page moved or was pictured.
    Browser {
        page: String,
        url: String,
        screenshot: Option<String>,
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
}

/// One source the provider named. Title may be empty; URL is not.
#[derive(Clone)]
pub struct Citation {
    pub url: String,
    pub title: String,
}

pub type Events = mpsc::UnboundedReceiver<Event>;

pub struct Reply<T>(oneshot::Sender<Result<T, Error>>);

impl<T> Reply<T> {
    pub fn send(self, value: T) {
        let _ = self.0.send(Ok(value));
    }

    /// A sink nobody is waiting on — kernel approvals answer over the socket.
    pub fn ignore() -> Self {
        let (tx, _) = oneshot::channel();
        Self(tx)
    }
}

pub struct Session {
    reader: tokio::task::JoinHandle<()>,
    out: mpsc::UnboundedSender<String>,
    pub session_id: String,
    pub cwd: PathBuf,
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
                                protocol, kernel, ..
                            } => Event::Handshake {
                                protocol: Some(*protocol),
                                kernel: kernel.clone(),
                            },
                            _ => Event::Handshake {
                                protocol: None,
                                kernel: String::new(),
                            },
                        };
                        if tx.send(hand).is_err() {
                            return;
                        }
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
                cwd: launch.place.path,
            },
            rx,
        ))
    }

    pub fn prompt(&self, content: &Prompt) -> Result<()> {
        // Every attachment goes over as a path, images included: the kernel's
        // projection loads image files itself and sends them as pixels
        // (arbos-engine `project::image_paths`). Filtering images out here
        // — a leftover from the base64 `parts` wire — made screenshots
        // arrive as text only. Absolute paths, since the agent's cwd is
        // the place, not wherever the file was picked from.
        self.send_frame(&Frame::User {
            agent: self.session_id.clone(),
            text: content.text.clone(),
            steer: false,
            attachments: content
                .attachments
                .iter()
                .map(|a| {
                    std::path::absolute(&a.path)
                        .unwrap_or_else(|_| a.path.clone())
                        .display()
                        .to_string()
                })
                .collect(),
        })
    }

    pub fn cancel(&self) -> Result<(), Error> {
        self.send_frame(&Frame::Stop {
            agent: self.session_id.clone(),
        })
        .map_err(|e| Error::internal_error().data(e.to_string()))
    }

    pub fn approval(&self, request_id: &str, approved: bool) -> Result<()> {
        self.send_frame(&Frame::Approve {
            agent: self.session_id.clone(),
            call_id: request_id.to_string(),
            allow: approved,
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
    pub fn configure(&self, provider: &str, api_base: &str, model: &str, api_key: &str, remember: bool) {
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
    pub fn rewind(&self, turn: u32, files: bool) {
        let _ = self.send_frame(&Frame::Rewind {
            agent: self.session_id.clone(),
            turn,
            files,
        });
    }

    /// Pause or resume the agent (`/pause`, `/resume`).
    pub fn set_paused(&self, paused: bool) {
        let _ = self.send_frame(&Frame::Pause {
            agent: self.session_id.clone(),
            paused,
        });
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
        Frame::Event { agent: id, event } if id == agent || agent.is_empty() => kernel_event(event),
        // Streamed text, one chunk per frame (the kernel's live path); the
        // whole step arrives later as an `event` with a seq, which
        // `merge_stream_text` folds into what the chunks built.
        Frame::AssistantDelta { agent: id, text } if id == agent || agent.is_empty() => {
            vec![Event::Update(SessionUpdate::AgentMessageChunk(text_chunk(
                text,
            )))]
        }
        Frame::Working { agent: id, secs } if id == agent || agent.is_empty() => {
            vec![Event::Working(secs)]
        }
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
        Frame::ThinkingDelta { agent: id, text } if id == agent || agent.is_empty() => {
            vec![Event::Update(SessionUpdate::AgentThoughtChunk(text_chunk(
                text,
            )))]
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
        // The desktop reads transcripts from the files; the replay that a
        // file-less client needs is not for it. Skipped here so a replayed
        // line is never appended a second time.
        Frame::Snapshot { .. }
        | Frame::Tree { .. }
        | Frame::Hello { .. }
        | Frame::Configure { .. }
        | Frame::Replayed { .. }
        | Frame::HistoryEnd { .. }
        // A newer kernel's frame: nothing to show, nothing to lose.
        | Frame::Unknown => Vec::new(),
        Frame::Rewound {
            agent: id,
            dropped,
            restored,
            ..
        } if id == agent => vec![Event::Rewound { dropped, restored }],
        // The kernel's answer to a bad ask (an unknown agent, a rewind it
        // cannot do): the reason belongs in the chat, not in a log.
        Frame::Error {
            agent: Some(id),
            detail,
        } if id == agent => vec![Event::Refused(detail)],
        Frame::Plan { agent: id, nodes } if id == agent => vec![Event::Plan(nodes)],
        Frame::Board {
            owner,
            action,
            panel,
            terminal_ids,
            cwd,
            title,
            url,
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

fn kernel_event(event: arbos_core::Event) -> Vec<Event> {
    use arbos_core::EventKind;
    let recorded = event.seq > 0;
    match event.kind {
        // A transcript line (tailed or replayed) is the step's final text;
        // a live emit without a seq is a delta (older kernels send those
        // as events too).
        EventKind::Assistant { text, .. } if recorded => vec![Event::AssistantFinal(text)],
        EventKind::Assistant { text, .. } => {
            vec![Event::Update(SessionUpdate::AgentMessageChunk(text_chunk(
                text,
            )))]
        }
        EventKind::Thinking { text } => {
            vec![Event::Update(SessionUpdate::AgentThoughtChunk(text_chunk(
                text,
            )))]
        }
        EventKind::Say { from, text } => vec![Event::Incoming { who: from, text }],
        EventKind::Notice { text, failed: true } => {
            vec![Event::TurnDone(Err(Error::internal_error().data(text)))]
        }
        EventKind::Notice { text, .. } => vec![Event::Aside(text)],
        // The turn was cut short; the pane says by whom (the fold line
        // picks the same text up).
        EventKind::Interrupted { detail } => vec![Event::Aside(
            crate::model::session::interrupt_label(&detail),
        )],
        EventKind::Ask {
            question, options, ..
        } => vec![Event::NeedQuestion {
            request_id: "ask".into(),
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
            tool.status = if rec.error.is_some() {
                ToolCallStatus::Failed
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
        EventKind::TurnComplete { usage } => usage
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
        "bash" | "run" | "exec" | "grep" | "find" | "tgrep" | "glob"
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
    obj.and_then(|obj| {
        ["path", "file", "target", "pattern", "query", "command"]
            .iter()
            .find_map(|key| obj.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|hint| !hint.is_empty())
            .map(str::to_owned)
    })
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
