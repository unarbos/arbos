//! Sub-agent job tabs (Jacob, 2026-10-25 19:41 UTC; [feedback subagent-job-tabs-mac-2026-10-25]):
//! every background job an agent starts is its own chat tab next to that agent's chat, in the
//! multi-chat tab system — the brief as the first message, the job's words, its tool steps as
//! system lines and the result as messages, live while it runs. Typing in the tab steers the job
//! (a reply to the job's ask in the parent chat, the wire the drawer's Steer uses); the composer's
//! Stop sends `stop { jobId }`. A finished job keeps its tab until the user closes it. Links to a
//! job — the status line, a tasks-drawer row — select the tab; a job started from a group chat
//! lives under the agent's own 1:1 chat and the link jumps there.
//!
//! The tab's chat id is `job:<jobId>`; its messages are synthesised here from the job's exchange
//! (`JobRollout`, `GET /projects/{id}/jobs/{jobId}[?after=seq]`), so the transcript, the presence
//! row and the composer need no new code paths. The chat itself still draws nothing for the job
//! (the hidden `job` row): the tab is where the job lives.

use crate::model::{
    Chat, ChatKind, Id, JobRollout, Message, MessageStatus, Part, Role, RolloutEntry, RolloutKind,
    StatusKind,
};

/// The id prefix of a job tab's chat.
pub const CHAT_PREFIX: &str = "job:";

pub fn chat_id(job_id: &str) -> Id {
    format!("{CHAT_PREFIX}{job_id}")
}

/// The job id behind a job tab's chat id.
pub fn job_id(chat_id: &str) -> Option<&str> {
    chat_id.strip_prefix(CHAT_PREFIX)
}

pub fn is_job_chat(chat_id: &str) -> bool {
    chat_id.starts_with(CHAT_PREFIX)
}

/// The tab entry for a job under `project_id`.
pub fn tab(project_id: &str, job_id: &str, title: &str, at: u64) -> Chat {
    Chat {
        id: chat_id(job_id),
        project_id: project_id.to_string(),
        title: if title.trim().is_empty() {
            "Job".to_string()
        } else {
            title.to_string()
        },
        is_pinned: false,
        is_system: false,
        unread: false,
        unread_count: 0,
        last_message_at: at,
        archived_at: None,
        kind: ChatKind::Job,
        participants: Vec::new(),
    }
}

/// The step line's words: "Read tripadvisor.com · 4.8 s" done, "Reading delsoulsurf.com…" live.
pub fn step_line(entry: &RolloutEntry) -> String {
    let words = entry
        .label
        .clone()
        .or_else(|| entry.text.clone())
        .unwrap_or_default();
    if entry.is_live() {
        format!("{words}…")
    } else {
        match entry.duration_label() {
            Some(d) => format!("{words} · {d}"),
            None => words,
        }
    }
}

fn base(chat: &str, id: String, role: Role, status: MessageStatus, at: u64) -> Message {
    Message {
        id,
        chat_id: chat.to_string(),
        role,
        sender: match role {
            Role::User => "user".into(),
            _ => "agent".into(),
        },
        status,
        turn: None,
        decision: None,
        subagent_id: None,
        subagent_name: None,
        handoff_messages: None,
        worker: false,
        job: None,
        hidden: false,
        agent_reply_to: None,
        reactions: Default::default(),
        in_reply_to: None,
        routine_name: None,
        sender_name: None,
        sender_icon: None,
        sender_handle: None,
        sender_avatar: None,
        sender_avatar_color: None,
        mentions: Vec::new(),
        reply_to: None,
        forwarded_from: None,
        reply_ref: None,
        asked_by_handle: None,
        owner_handle: None,
        usage: None,
        return_contract: None,
        waiting_user: false,
        waiting_approval: false,
        model: None,
        effort: None,
        created_at: at,
        completed_at: Some(at),
        call_id: None,
        activity: None,
        pending: false,
        parts: Vec::new(),
        hidden_parts: Vec::new(),
        greeting: false,
    }
}

fn text_part(id: &str, text: &str) -> Part {
    Part::Text {
        id: format!("{id}-t"),
        text: text.to_string(),
    }
}

/// A steer typed in the tab, echoed at once (pending until the box's entry replaces it);
/// the same row, re-homed, is the parent chat's reply under the ask.
pub fn typed_steer(chat_id: &str, message_id: &str, text: &str, ask: &str) -> Message {
    let now = crate::model::now_ms();
    let mut m = base(
        chat_id,
        message_id.to_string(),
        Role::User,
        MessageStatus::Done,
        now,
    );
    m.reply_to = Some(ask.to_string());
    m.pending = true;
    m.parts.push(text_part(message_id, text));
    m
}

/// The tab's transcript from the job's exchange: the brief (and every steer) as a user
/// line, the job's words and its result as agent lines, each tool step as a system line,
/// and — while the job runs — a wordless streaming agent row whose `activity` is the live
/// step, so the presence row reads "Reading delsoulsurf.com" and the composer shows Stop.
/// `pending` are steers typed in the tab the box has not echoed yet (dropped once an
/// entry with the same words arrives).
pub fn messages(rollout: &JobRollout, pending: &[Message]) -> Vec<Message> {
    let chat = chat_id(&rollout.id);
    let mut out: Vec<Message> = Vec::new();
    let mut live_step: Option<String> = None;
    if rollout.unavailable {
        // A relay before the endpoint: say so once; the tab still steers and stops.
        let id = format!("{chat}:unavailable");
        let mut m = base(
            &chat,
            id.clone(),
            Role::System,
            MessageStatus::Done,
            crate::model::now_ms(),
        );
        m.sender = "system".into();
        m.parts.push(text_part(
            &id,
            "This task's transcript is not available on this relay yet.",
        ));
        out.push(m);
    }
    for e in &rollout.entries {
        let id = format!("{chat}:{}", e.seq);
        let at = if e.at > 0 {
            e.at
        } else {
            rollout.started_at.unwrap_or(0)
        };
        match e.kind {
            RolloutKind::Prompt | RolloutKind::Steer => {
                let mut m = base(&chat, id.clone(), Role::User, MessageStatus::Done, at);
                m.parts
                    .push(text_part(&id, e.text.as_deref().unwrap_or_default()));
                out.push(m);
            }
            RolloutKind::Text | RolloutKind::Result => {
                let mut m = base(&chat, id.clone(), Role::Assistant, MessageStatus::Done, at);
                if let Some(t) = e.text.as_deref().filter(|t| !t.trim().is_empty()) {
                    m.parts.push(text_part(&id, t));
                }
                if e.kind == RolloutKind::Result {
                    m.parts.push(Part::Status {
                        id: format!("{id}-s"),
                        code: match e.status.as_deref() {
                            Some("failed") => StatusKind::Failed,
                            Some("cancelled") => StatusKind::Stopped,
                            _ => StatusKind::Done,
                        },
                        message: match e.status.as_deref() {
                            Some("failed") => "Failed".into(),
                            Some("cancelled") => "Stopped".into(),
                            _ => "Done".into(),
                        },
                    });
                    if e.status.as_deref() == Some("cancelled") {
                        m.status = MessageStatus::Cancelled;
                    }
                }
                out.push(m);
            }
            RolloutKind::Tool => {
                if e.is_live() {
                    live_step = e.label.clone().or_else(|| e.text.clone());
                }
                let mut m = base(&chat, id.clone(), Role::System, MessageStatus::Done, at);
                m.sender = "system".into();
                m.parts.push(text_part(&id, &step_line(e)));
                out.push(m);
            }
            RolloutKind::Other => {}
        }
    }
    // Steers typed here that the box has not echoed as entries yet.
    for p in pending {
        let echoed = rollout.entries.iter().any(|e| {
            e.kind == RolloutKind::Steer
                && e.text.as_deref().map(str::trim) == Some(p.plain_text().trim())
        });
        if !echoed {
            out.push(p.clone());
        }
    }
    if rollout.is_running() {
        let now = crate::model::now_ms();
        let mut live = base(
            &chat,
            format!("{chat}:live"),
            Role::Assistant,
            MessageStatus::Streaming,
            now,
        );
        live.completed_at = None;
        live.activity = Some(live_step.unwrap_or_else(|| {
            if rollout.step.trim().is_empty() {
                format!("{} is working", rollout.title)
            } else {
                rollout.step.clone()
            }
        }));
        live.parts.push(Part::Status {
            id: format!("{chat}:live-s"),
            code: StatusKind::Working,
            message: "Working…".into(),
        });
        out.push(live);
    }
    out
}

#[cfg(test)]
mod tests {
    //! The tab's transcript follows the exchange: brief → user line, words → agent lines,
    //! steps → system lines ("· 4.8 s" done, "…" live), result → agent line with a Done
    //! status, and a streaming row with the live step's words while the job runs.
    use super::*;
    use crate::model::{JobState, RolloutParent};

    fn entry(
        seq: u64,
        kind: RolloutKind,
        words: &str,
        status: Option<&str>,
        dur: Option<u64>,
    ) -> RolloutEntry {
        let tool = kind == RolloutKind::Tool;
        RolloutEntry {
            seq,
            at: 1_700_000_000_000 + seq * 1000,
            kind,
            text: (!tool).then(|| words.to_string()),
            label: tool.then(|| words.to_string()),
            name: tool.then(|| "tool".to_string()),
            status: status.map(str::to_string),
            duration_ms: dur,
        }
    }

    fn rollout(entries: Vec<RolloutEntry>, ended: bool) -> JobRollout {
        JobRollout {
            id: "j1".into(),
            title: "Searching for surf schools".into(),
            kind: "research".into(),
            state: JobState::Running,
            step: "Reading tripadvisor.com".into(),
            elapsed_s: 41,
            started_at: Some(1_700_000_000_000),
            ended_at: ended.then_some(1_700_000_400_000),
            chat_id: Some("c1".into()),
            ask_message_id: Some("m1".into()),
            parent: RolloutParent {
                project_id: "p1".into(),
                name: "Affine".into(),
            },
            brief: "Find the 3 best surf schools.".into(),
            entries,
            tool_calls: 2,
            next_seq: 10,
            unavailable: false,
        }
    }

    #[test]
    fn a_running_job_reads_as_brief_words_steps_and_a_live_row() {
        let r = rollout(
            vec![
                entry(
                    1,
                    RolloutKind::Prompt,
                    "Find the 3 best surf schools.",
                    None,
                    None,
                ),
                entry(2, RolloutKind::Text, "On it.", None, None),
                entry(
                    3,
                    RolloutKind::Tool,
                    "Read tripadvisor.com",
                    Some("done"),
                    Some(4800),
                ),
                entry(
                    4,
                    RolloutKind::Tool,
                    "Reading delsoulsurf.com",
                    Some("running"),
                    None,
                ),
            ],
            false,
        );
        let m = messages(&r, &[]);
        assert_eq!(m.len(), 5);
        assert_eq!(
            (m[0].role, m[0].plain_text().as_str()),
            (Role::User, "Find the 3 best surf schools.")
        );
        assert_eq!(
            (m[1].role, m[1].plain_text().as_str()),
            (Role::Assistant, "On it.")
        );
        assert_eq!(
            (m[2].role, m[2].plain_text().as_str()),
            (Role::System, "Read tripadvisor.com · 4.8 s")
        );
        assert_eq!(m[3].plain_text(), "Reading delsoulsurf.com…");
        assert!(
            m[4].is_streaming(),
            "a live row keeps the presence row and Stop up"
        );
        assert_eq!(
            m[4].current_activity().as_deref(),
            Some("Reading delsoulsurf.com")
        );
        assert!(Message::chat_is_actively_working(
            &m,
            crate::model::now_ms()
        ));
        assert_eq!(m[0].chat_id, "job:j1");
    }

    #[test]
    fn a_finished_job_ends_with_the_result_and_no_live_row() {
        let r = rollout(
            vec![
                entry(
                    1,
                    RolloutKind::Prompt,
                    "Find the 3 best surf schools.",
                    None,
                    None,
                ),
                entry(
                    2,
                    RolloutKind::Tool,
                    "Read tripadvisor.com",
                    Some("done"),
                    Some(4800),
                ),
                entry(
                    3,
                    RolloutKind::Result,
                    "Blue Mystic is the best value.",
                    Some("done"),
                    Some(372_000),
                ),
            ],
            true,
        );
        let m = messages(&r, &[]);
        assert_eq!(m.len(), 3);
        assert_eq!(m[2].role, Role::Assistant);
        assert_eq!(m[2].plain_text(), "Blue Mystic is the best value.");
        assert!(!m.iter().any(Message::is_streaming));
        assert!(!Message::chat_is_actively_working(
            &m,
            crate::model::now_ms()
        ));
    }

    #[test]
    fn a_typed_steer_shows_until_the_box_echoes_it() {
        let r0 = rollout(
            vec![entry(1, RolloutKind::Prompt, "Find schools.", None, None)],
            false,
        );
        let mut typed = base(
            "job:j1",
            "local-1".into(),
            Role::User,
            MessageStatus::Done,
            1,
        );
        typed
            .parts
            .push(text_part("local-1", "only group lessons under $70"));
        typed.pending = true;
        let m = messages(&r0, std::slice::from_ref(&typed));
        assert!(
            m.iter().any(|x| x.id == "local-1"),
            "the typed steer shows at once"
        );
        let r1 = rollout(
            vec![
                entry(1, RolloutKind::Prompt, "Find schools.", None, None),
                entry(
                    2,
                    RolloutKind::Steer,
                    "only group lessons under $70",
                    None,
                    None,
                ),
            ],
            false,
        );
        let m = messages(&r1, std::slice::from_ref(&typed));
        assert!(!m.iter().any(|x| x.id == "local-1"), "the echo replaces it");
        assert_eq!(m[1].role, Role::User);
    }

    #[test]
    fn ids_round_trip() {
        assert_eq!(chat_id("01M"), "job:01M");
        assert_eq!(job_id("job:01M"), Some("01M"));
        assert_eq!(job_id("01M"), None);
        assert!(is_job_chat("job:x") && !is_job_chat("c1"));
        assert_eq!(tab("p1", "j1", "", 5).title, "Job");
        assert_eq!(tab("p1", "j1", "Fixing tests", 5).kind, ChatKind::Job);
    }
}

#[cfg(test)]
mod render_tests {
    //! The real `Shell` headless (the `transcript_smoke_tests` mount): a running job's tab
    //! renders the brief, the step lines and the live presence row with Stop; a finished
    //! job's tab ends with the result and no live row; the status line's link and a
    //! drawer row select the job's tab; from a group chat the link jumps to the agent's
    //! space first; typing in the tab steers (a reply under the ask in the parent chat);
    //! closing the tab returns to Main.
    use futures::future::BoxFuture;
    use gpui::{
        point, AppContext as _, Bounds, Entity, Modifiers, Pixels, TestAppContext,
        VisualTestContext,
    };
    use tokio::sync::mpsc;

    use super::*;
    use crate::app::AppState;
    use crate::model::{AgentMember, Job, JobState, Project, RolloutParent, SpaceKind};
    use crate::sync::{ClientCommand, SyncEvent, SyncSource};
    use crate::ui::motion::PulseClock;
    use crate::ui::shell::Shell;
    use crate::ui::theme::Theme;

    struct Idle;

    impl SyncSource for Idle {
        fn name(&self) -> &'static str {
            "idle"
        }

        fn run(
            self: Box<Self>,
            _events: mpsc::UnboundedSender<SyncEvent>,
            _commands: mpsc::UnboundedReceiver<ClientCommand>,
        ) -> BoxFuture<'static, anyhow::Result<()>> {
            Box::pin(std::future::pending())
        }
    }

    fn user(chat: &str, id: &str, text: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": chat, "role": "user", "sender": "user", "status": "done",
            "createdAt": at, "completedAt": at,
            "parts": [{ "id": format!("{id}-t"), "kind": "text", "text": text }],
        }))
        .expect("message")
    }

    fn job_row(chat: &str, id: &str, job_id: &str, title: &str, at: u64) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "chatId": chat, "role": "assistant", "sender": "box:1",
            "status": "streaming", "createdAt": at,
            "job": { "id": job_id, "title": title, "kind": "research", "touches": [] },
            "worker": true, "hidden": true, "inReplyTo": "m1", "activity": "Reading tripadvisor.com",
            "parts": [{ "id": format!("{id}-s"), "kind": "status", "code": "working", "message": "Working…" }],
        }))
        .expect("message")
    }

    fn job(id: &str, title: &str, chat: &str) -> Job {
        Job {
            id: id.into(),
            title: title.into(),
            state: JobState::Running,
            step: "Reading tripadvisor.com".into(),
            elapsed_s: 41,
            ask_message_id: Some("m1".into()),
            chat_id: Some(chat.into()),
        }
    }

    fn entry(
        seq: u64,
        kind: RolloutKind,
        words: &str,
        status: Option<&str>,
        dur: Option<u64>,
    ) -> RolloutEntry {
        let tool = kind == RolloutKind::Tool;
        RolloutEntry {
            seq,
            at: 1_700_000_000_000 + seq * 1000,
            kind,
            text: (!tool).then(|| words.to_string()),
            label: tool.then(|| words.to_string()),
            name: tool.then(|| "tool".to_string()),
            status: status.map(str::to_string),
            duration_ms: dur,
        }
    }

    fn rollout(job_id: &str, chat: &str, finished: bool) -> JobRollout {
        let mut entries = vec![
            entry(
                1,
                RolloutKind::Prompt,
                "Find the 3 best surf schools near Santa Teresa.",
                None,
                None,
            ),
            entry(
                2,
                RolloutKind::Text,
                "On it — pulling ratings and prices.",
                None,
                None,
            ),
            entry(
                3,
                RolloutKind::Tool,
                "Read tripadvisor.com",
                Some("done"),
                Some(4800),
            ),
        ];
        if finished {
            entries.push(entry(
                4,
                RolloutKind::Result,
                "Blue Mystic is the best value.",
                Some("done"),
                Some(372_000),
            ));
        } else {
            entries.push(entry(
                4,
                RolloutKind::Tool,
                "Reading delsoulsurf.com",
                Some("running"),
                None,
            ));
        }
        JobRollout {
            id: job_id.into(),
            title: "Searching for surf schools".into(),
            kind: "research".into(),
            state: JobState::Running,
            step: "Reading delsoulsurf.com".into(),
            elapsed_s: 51,
            started_at: Some(1_700_000_000_000),
            ended_at: finished.then_some(1_700_000_372_000),
            chat_id: Some(chat.into()),
            ask_message_id: Some("m1".into()),
            parent: RolloutParent {
                project_id: "p1".into(),
                name: "Affine".into(),
            },
            brief: String::new(),
            entries,
            tool_calls: 2,
            next_seq: 5,
            unavailable: false,
        }
    }

    /// Agent `p1` ("Affine") with chat `c1`, plus group `g1` ("Launch week", a people space
    /// holding Affine) with chat `gc1`; the user asked in both and Affine runs job `j1` from
    /// `c1` and `j2` from the group.
    fn mount(
        cx: &mut TestAppContext,
    ) -> (
        tokio::runtime::Runtime,
        Entity<AppState>,
        Entity<Shell>,
        &mut VisualTestContext,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio");
        let handle = rt.handle().clone();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, handle);
            cx.set_global(Theme::dark(crate::ui::theme::Surface::Matte));
            if !cx.has_global::<PulseClock>() {
                PulseClock::init(cx);
            }
        });
        let state = cx.new(|cx| AppState::new(Box::new(Idle), cx));
        let now = crate::model::now_ms();
        state.update(cx, |s, cx| {
            s.projects.push(Project {
                id: "p1".into(),
                name: "Affine".into(),
                kind: SpaceKind::Project,
                ..Project::default()
            });
            s.projects.push(Project {
                id: "g1".into(),
                name: "Launch week".into(),
                kind: SpaceKind::Group,
                agents: vec![AgentMember {
                    project_id: "p1".into(),
                    name: "Affine".into(),
                    handle: "affine".into(),
                    ..Default::default()
                }],
                ..Project::default()
            });
            let chat = |id: &str, pid: &str, title: &str| -> Chat {
                serde_json::from_value(serde_json::json!({
                    "id": id, "project_id": pid, "title": title, "is_pinned": true,
                    "is_system": false, "unread": false, "last_message_at": now,
                }))
                .expect("chat")
            };
            s.chats.insert("p1".into(), vec![chat("c1", "p1", "Main")]);
            s.chats
                .insert("g1".into(), vec![chat("gc1", "g1", "Launch week")]);
            s.messages.insert(
                "c1".into(),
                vec![
                    user(
                        "c1",
                        "m1",
                        "Find the 3 best surf schools near Santa Teresa.",
                        now - 60_000,
                    ),
                    job_row("c1", "m2", "j1", "Searching for surf schools", now - 41_000),
                ],
            );
            s.messages.insert(
                "gc1".into(),
                vec![
                    user("gc1", "m1", "Affine, compare hosting prices.", now - 60_000),
                    job_row("gc1", "m3", "j2", "Comparing hosting prices", now - 20_000),
                ],
            );
            s.selected_project = Some("p1".into());
            s.active_tab.insert("p1".into(), "c1".into());
            s.active_tab.insert("g1".into(), "gc1".into());
            cx.notify();
        });
        let (shell, vcx) = cx.add_window_view(|window, cx| Shell::new(state.clone(), window, cx));
        vcx.run_until_parked();
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::ProjectJobs {
                    project_id: "p1".into(),
                    jobs: vec![
                        job("j1", "Searching for surf schools", "c1"),
                        job("j2", "Comparing hosting prices", "gc1"),
                    ],
                },
                cx,
            )
        });
        vcx.run_until_parked();
        (rt, state, shell, vcx)
    }

    fn settle(shell: &Entity<Shell>, vcx: &mut VisualTestContext) {
        for _ in 0..3 {
            shell.update(vcx, |_, cx| cx.notify());
            vcx.run_until_parked();
        }
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<ClientCommand>) -> Vec<ClientCommand> {
        let mut out = Vec::new();
        while let Ok(c) = rx.try_recv() {
            out.push(c);
        }
        out
    }

    fn centre(b: Bounds<Pixels>) -> gpui::Point<Pixels> {
        point(b.left() + b.size.width / 2.0, b.top() + b.size.height / 2.0)
    }

    fn rows_of(shell: &Entity<Shell>, vcx: &VisualTestContext) -> Vec<(String, String)> {
        shell.read_with(vcx, |sh, cx| {
            sh.transcript
                .read(cx)
                .rows
                .iter()
                .map(|r| {
                    (
                        r.message_id.clone(),
                        format!("{:?}", std::mem::discriminant(&r.kind)),
                    )
                })
                .collect()
        })
    }

    #[gpui::test]
    fn a_running_jobs_tab_shows_the_brief_the_steps_and_the_live_row(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        // Every listed job has its tab after the agent's chats.
        let tabs: Vec<String> = state.read_with(&*vcx, |s, _| {
            s.visible_tabs("p1").iter().map(|c| c.id.clone()).collect()
        });
        assert_eq!(tabs, vec!["c1", "job:j1", "job:j2"]);
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        state.update(vcx, |s, cx| s.open_job_tab("j1", cx));
        settle(&shell, vcx);
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.active_chat_id()).as_deref(),
            Some("job:j1")
        );
        let sent = drain(&mut commands);
        assert!(
            sent.iter().any(|c| matches!(c, ClientCommand::RolloutRefresh { job_id, after: None, .. } if job_id == "j1")),
            "opening the tab fetches the whole document: {sent:?}"
        );
        state.update(vcx, |s, cx| {
            s.apply_rollout("j1", rollout("j1", "c1", false), cx)
        });
        settle(&shell, vcx);
        let rows = rows_of(&shell, &*vcx);
        let ids: Vec<&str> = rows.iter().map(|(id, _)| id.as_str()).collect();
        assert!(
            ids.contains(&"job:j1:1"),
            "the brief is the first message: {ids:?}"
        );
        assert!(ids.contains(&"job:j1:2"), "the job's words: {ids:?}");
        assert!(
            ids.contains(&"job:j1:3") && ids.contains(&"job:j1:4"),
            "the steps as lines: {ids:?}"
        );
        assert!(vcx.debug_bounds("chat-bubble-assistant").is_some());
        assert!(
            vcx.debug_bounds("presence-row").is_some(),
            "the live step keeps the presence row up"
        );
        assert!(
            state.read_with(&*vcx, |s, _| s.chat_is_streaming("job:j1")),
            "the composer shows Stop"
        );
        // The pill names the tab.
        let title = state.read_with(&*vcx, |s, _| {
            s.visible_tabs("p1")
                .iter()
                .find(|c| c.id == "job:j1")
                .map(|c| c.title.clone())
        });
        assert_eq!(title.as_deref(), Some("Searching for surf schools"));
        // Stop in the tab halts that job only.
        state.update(vcx, |s, cx| s.stop(cx));
        let sent = drain(&mut commands);
        assert!(
            sent.iter().any(|c| matches!(c, ClientCommand::StopJob { chat_id, job_id, .. } if chat_id == "c1" && job_id == "j1")),
            "{sent:?}"
        );
        assert!(
            !sent.iter().any(|c| matches!(c, ClientCommand::Stop { .. })),
            "never the whole turn"
        );
    }

    #[gpui::test]
    fn a_finished_jobs_tab_ends_with_the_result_and_stays_until_closed(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| s.open_job_tab("j1", cx));
        state.update(vcx, |s, cx| {
            s.apply_rollout("j1", rollout("j1", "c1", true), cx)
        });
        // The listing no longer names the job: the tab stays.
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::ProjectJobs {
                    project_id: "p1".into(),
                    jobs: vec![job("j2", "Comparing hosting prices", "gc1")],
                },
                cx,
            )
        });
        settle(&shell, vcx);
        let ids: Vec<String> = rows_of(&shell, &*vcx)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(
            ids.iter().any(|id| id == "job:j1:4"),
            "the result is the last message: {ids:?}"
        );
        assert!(!ids.iter().any(|id| id == "job:j1:live"));
        assert!(
            vcx.debug_bounds("presence-row").is_none(),
            "nothing works any more"
        );
        assert!(!state.read_with(&*vcx, |s, _| s.chat_is_streaming("job:j1")));
        assert!(state.read_with(&*vcx, |s, _| s
            .visible_tabs("p1")
            .iter()
            .any(|c| c.id == "job:j1")));
        // Close: back to Main, the tab is gone and a later listing does not bring it back.
        assert!(state.update(vcx, |s, cx| s.close_tab("job:j1".into(), cx)));
        settle(&shell, vcx);
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.active_chat_id()).as_deref(),
            Some("c1")
        );
        state.update(vcx, |s, cx| {
            s.apply(
                SyncEvent::ProjectJobs {
                    project_id: "p1".into(),
                    jobs: vec![job("j1", "Searching for surf schools", "c1")],
                },
                cx,
            )
        });
        assert!(!state.read_with(&*vcx, |s, _| s
            .visible_tabs("p1")
            .iter()
            .any(|c| c.id == "job:j1")));
    }

    #[gpui::test]
    fn the_status_line_and_a_drawer_row_select_the_jobs_tab(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        settle(&shell, vcx);
        // The words are a link while jobs are open; a click selects the newest job's tab.
        let link = vcx
            .debug_bounds("presence-link-rest")
            .expect("the status line is a link");
        vcx.simulate_click(centre(link), Modifiers::default());
        settle(&shell, vcx);
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.active_chat_id()).as_deref(),
            Some("job:j1")
        );
        // Back in Main, a drawer row opens its own job's tab.
        state.update(vcx, |s, cx| s.select_tab("c1".into(), cx));
        settle(&shell, vcx);
        let chip = vcx.debug_bounds("tasks-chip").expect("chip");
        vcx.simulate_click(centre(chip), Modifiers::default());
        settle(&shell, vcx);
        let row = vcx.debug_bounds("tasks-row-title-0").expect("drawer row");
        vcx.simulate_click(centre(row), Modifiers::default());
        settle(&shell, vcx);
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.active_chat_id()).as_deref(),
            Some("job:j1")
        );
        assert!(
            state.read_with(&*vcx, |s, _| s.tasks_open.is_none()),
            "the drawer closed"
        );
        // In the job tab the line is not a link.
        state.update(vcx, |s, cx| {
            s.apply_rollout("j1", rollout("j1", "c1", false), cx)
        });
        settle(&shell, vcx);
        assert!(vcx.debug_bounds("presence-link-rest").is_none());
    }

    #[gpui::test]
    fn from_a_group_the_link_jumps_to_the_agents_space_and_the_jobs_tab(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        state.update(vcx, |s, cx| s.select_project("g1".into(), cx));
        settle(&shell, vcx);
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.active_chat_id()).as_deref(),
            Some("gc1")
        );
        let open: Vec<String> = state.read_with(&*vcx, |s, _| {
            s.open_jobs("gc1").iter().map(|j| j.id.clone()).collect()
        });
        assert_eq!(
            open,
            vec!["j2"],
            "the group lists the job its agent started there"
        );
        state.update(vcx, |s, cx| s.open_newest_job_tab("gc1".into(), cx));
        settle(&shell, vcx);
        assert_eq!(
            state
                .read_with(&*vcx, |s, _| s.selected_project.clone())
                .as_deref(),
            Some("p1")
        );
        assert_eq!(
            state.read_with(&*vcx, |s, _| s.active_chat_id()).as_deref(),
            Some("job:j2")
        );
    }

    #[gpui::test]
    fn typing_in_the_tab_steers_the_job_as_a_reply_under_the_ask(cx: &mut TestAppContext) {
        let (_rt, state, shell, vcx) = mount(cx);
        let mut commands = state.update(vcx, |s, _| s.take_command_channel());
        state.update(vcx, |s, cx| s.open_job_tab("j1", cx));
        state.update(vcx, |s, cx| {
            s.apply_rollout("j1", rollout("j1", "c1", false), cx)
        });
        settle(&shell, vcx);
        let _ = drain(&mut commands);
        state.update(vcx, |s, cx| {
            s.send(
                "only group lessons under $70".into(),
                Vec::new(),
                None,
                Vec::new(),
                cx,
            )
        });
        settle(&shell, vcx);
        let sent = drain(&mut commands);
        let send = sent.iter().find_map(|c| match c {
            ClientCommand::Send {
                chat_id,
                reply_to,
                text,
                ..
            } => Some((chat_id.clone(), reply_to.clone(), text.clone())),
            _ => None,
        });
        let (chat_id, reply_to, text) =
            send.unwrap_or_else(|| panic!("expected Send, got {sent:?}"));
        assert_eq!(chat_id, "c1", "the steer goes to the parent chat");
        assert_eq!(reply_to.as_deref(), Some("m1"), "as a reply under the ask");
        assert_eq!(text, "only group lessons under $70");
        let tab_text: Vec<String> = state.read_with(&*vcx, |s, _| {
            s.messages_of("job:j1")
                .iter()
                .map(Message::plain_text)
                .collect()
        });
        assert!(
            tab_text.iter().any(|t| t == "only group lessons under $70"),
            "echoed in the tab: {tab_text:?}"
        );
        let parent_text: Vec<String> = state.read_with(&*vcx, |s, _| {
            s.messages_of("c1")
                .iter()
                .map(Message::plain_text)
                .collect()
        });
        assert!(
            parent_text
                .iter()
                .any(|t| t == "only group lessons under $70"),
            "and in the parent chat"
        );
    }
}
