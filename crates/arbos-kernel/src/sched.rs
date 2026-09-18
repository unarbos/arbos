use arbos_core::{AgentId, Place, Wake, files::load_agent};
use arbos_engine::{BoxFuture, Host, Registry, TurnControl, turn};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

use crate::hooks::KernelHooks;

/// Live children one agent may have at once, by default. Jacob's Projects
/// run wide (the Cursor Projects post: "more subagents in parallel than
/// your laptop could support"); 8 stalled a coordinator on its ninth
/// spawn. `max_children` in config.toml overrides, up to MAX_CHILDREN_CAP.
pub const MAX_CHILDREN: usize = 24;
/// The most `max_children` may be set to.
pub const MAX_CHILDREN_CAP: usize = 256;
pub const MAX_DEPTH: usize = 3;

/// One in-flight turn per agent, each with its own control handle.
pub struct Scheduler {
    pub in_flight: Arc<Mutex<HashMap<String, TurnControl>>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// A scheduler over the hooks' table, so `say mode=steer` and the
    /// attach loop reach the same live turns.
    pub fn sharing(in_flight: Arc<Mutex<HashMap<String, TurnControl>>>) -> Self {
        Self { in_flight }
    }

    pub fn has_job(&self, id: &str) -> bool {
        self.in_flight.lock().unwrap().contains_key(id)
    }

    fn control(&self, id: &str) -> Option<TurnControl> {
        self.in_flight.lock().unwrap().get(id).cloned()
    }

    pub fn stop(&self, id: &str) {
        if let Some(c) = self.control(id) {
            c.stop();
        }
    }

    pub fn stop_for(&self, id: &str, reason: &str) {
        if let Some(c) = self.control(id) {
            c.stop_for(reason);
        }
    }

    /// Compact before the next model call. Returns false when no turn is
    /// running for the agent; the caller then wakes one to do it.
    pub fn request_compact(&self, id: &str) -> bool {
        match self.control(id) {
            Some(c) => {
                c.request_compact();
                true
            }
            None => false,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &self,
        place: Place,
        wake: Wake,
        host: Host,
        registry: Arc<Registry>,
        grep: Arc<dyn arbos_engine::Grep>,
        hooks: Arc<KernelHooks>,
        done: mpsc::UnboundedSender<String>,
    ) {
        let id = wake.agent.as_str().to_string();
        if self.has_job(&id) {
            return;
        }
        let control = TurnControl::new();
        let started = std::time::Instant::now();
        crate::klog::info(
            "turn_start",
            Some(&id),
            format!("wake={}", wake.kind.as_str()),
        );
        self.in_flight
            .lock()
            .unwrap()
            .insert(id.clone(), control.clone());
        tokio::spawn(async move {
            // However this task ends — a return, a panic, an abort — the
            // serve loop hears that the turn is over. Without the guard a
            // panic anywhere on the turn's own task (a poisoned lock, an
            // index past the end) left the agent in `running` for good: no
            // `turn_complete`, no frame, every later message queued behind
            // a turn that would never end, and nothing for the user but
            // the working line. That is the "finished and unnoticed" class
            // in its purest form, so it is closed here rather than at each
            // site that could panic.
            let _done = DoneGuard {
                id: id.clone(),
                done,
            };
            let agent = match load_agent(&place, &wake.agent).map(|mut a| {
                arbos_core::project::apply_role(&place, &mut a);
                a
            }) {
                Ok(a) => a,
                Err(e) => {
                    crate::klog::error("load_agent", Some(&id), format!("{e:#}"));
                    return;
                }
            };
            if agent.paused {
                return;
            }
            // An outside ACP program runs this kind's turns (P-14).
            if crate::acp_worker::command_for(&place, &agent).is_some() {
                let res = crate::acp_worker::turn(place, agent, wake, hooks, control).await;
                match &res {
                    Ok(()) => crate::klog::info(
                        "turn_end",
                        Some(&id),
                        format!("acp {:.1}s", started.elapsed().as_secs_f64()),
                    ),
                    Err(e) => crate::klog::error("turn_error", Some(&id), format!("acp: {e:#}")),
                }
                return;
            }
            let wrap = TurnHooks {
                inner: Arc::clone(&hooks),
                agent: wake.agent.clone(),
            };
            let transcript = hooks.layout(&id).transcript();
            let (wake_kind, wake_text) = (wake.kind.clone(), wake.text.clone());
            let opts = arbos_engine::TurnOpts {
                place,
                agent,
                wake,
                host,
                registry,
                grep,
                hooks: Arc::new(wrap),
                control,
            };
            // The turn runs on a task of its own so a panic in it is a
            // `JoinError` here, with its message, rather than the end of
            // this task.
            // Test knob: `ARBOS_TEST_PANIC_TURN=<agent>` panics that
            // agent's first turn in this process, so the guard below is
            // driven rather than trusted.
            let panic_now = std::env::var("ARBOS_TEST_PANIC_TURN").is_ok_and(|who| who == id)
                && !PANIC_FIRED.swap(true, std::sync::atomic::Ordering::SeqCst);
            let res = tokio::spawn(async move {
                if panic_now {
                    panic!("ARBOS_TEST_PANIC_TURN: the turn task panicked on purpose");
                }
                turn(opts).await
            })
            .await;
            match res {
                Ok(Ok(())) => crate::klog::info(
                    "turn_end",
                    Some(&id),
                    format!("{:.1}s", started.elapsed().as_secs_f64()),
                ),
                Ok(Err(e)) => {
                    crate::klog::error(
                        "turn_error",
                        Some(&id),
                        format!("{e:#} after {:.1}s", started.elapsed().as_secs_f64()),
                    );
                    // The error was only a log line: the transcript ended
                    // on whatever came before it, the turn folder closed as
                    // "success (no reply)", and a window drew a turn that
                    // simply stopped. Now the record ends in words, like a
                    // panic's does — and when the transcript itself cannot
                    // be written (the store read-only, the disk full), the
                    // words go to the attached windows live, and a person
                    // whose message never reached the record is told where
                    // it is kept and to send it again.
                    crate::plan::note_turn_error(&hooks, &id, &format!("{e:#}"));
                    turn_error_said(
                        &hooks,
                        &id,
                        &transcript,
                        wake_kind,
                        wake_text.as_deref(),
                        &e,
                    );
                }
                Err(e) => {
                    let why = panic_text(e);
                    crate::klog::error(
                        "turn_panicked",
                        Some(&id),
                        format!("{why} after {:.1}s", started.elapsed().as_secs_f64()),
                    );
                    // The record ends here, in words, so the window shows
                    // why the turn stopped and the next boot does not
                    // replay the wake as unfinished. The turn folder
                    // remembers the panic too, and when the transcript
                    // cannot take the words they go to the windows live
                    // (the same as a turn's Err, #408).
                    crate::plan::note_turn_error(&hooks, &id, &format!("panic: {why}"));
                    let notice = arbos_core::Event::new(arbos_core::EventKind::Notice {
                        text: format!(
                            "The kernel hit an internal error in this turn and ended it: {why}. What ran before it stands; send again to go on. (kernel.log has the detail.)"
                        ),
                        failed: true,
                    });
                    if let Err(write_err) = arbos_core::append_events(
                        &transcript,
                        &[
                            notice.clone(),
                            arbos_core::Event::new(arbos_core::EventKind::TurnComplete {
                                usage: None,
                            }),
                        ],
                    ) {
                        crate::klog::error(
                            "turn_panicked_unrecorded",
                            Some(&id),
                            format!("the transcript would not take the notice: {write_err:#}"),
                        );
                        hooks.broadcast(arbos_core::wire::Frame::Event {
                            agent: id.to_string(),
                            event: notice,
                        });
                    }
                }
            }
        });
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

static PANIC_FIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tells the serve loop the turn is over when dropped: on return, on
/// panic (unwinding runs drops), on abort.
struct DoneGuard {
    id: String,
    done: mpsc::UnboundedSender<String>,
}

impl Drop for DoneGuard {
    fn drop(&mut self) {
        let _ = self.done.send(std::mem::take(&mut self.id));
    }
}

/// A turn that returned an error, said: a failed notice and
/// `turn_complete` on the transcript unless the turn already closed
/// itself; the same notice live to attached windows when the transcript
/// cannot take it; and, for a user's turn whose words never landed, the
/// place they are kept.
fn turn_error_said(
    hooks: &Arc<KernelHooks>,
    id: &str,
    transcript: &std::path::Path,
    wake_kind: arbos_core::WakeKind,
    wake_text: Option<&str>,
    e: &anyhow::Error,
) {
    use arbos_core::{Event, EventKind, wire::Frame};
    let events = arbos_core::load_transcript(transcript).unwrap_or_default();
    if matches!(
        events.last().map(|ev| &ev.kind),
        Some(EventKind::TurnComplete { .. })
    ) {
        return;
    }
    let words_lost = wake_kind == arbos_core::WakeKind::User
        && wake_text.is_some_and(|t| {
            !events
                .iter()
                .rev()
                .take(50)
                .any(|ev| matches!(&ev.kind, EventKind::User { text, .. } if text == t))
        });
    let mut text = format!(
        "The turn ended on an error: {e:#}. What ran before it stands; send again to go on. (kernel.log has the detail.)"
    );
    if words_lost && let Some(t) = wake_text {
        text = format!(
            "Your message did not reach the record: {e:#}. It is kept under .arbos/agents/{id}/turns/ as the cause of this turn — \"{}\" — send it again once the store can be written.",
            arbos_core::text::clip(t.trim(), 120)
        );
    }
    let notice = Event::new(EventKind::Notice { text, failed: true });
    let close = Event::new(EventKind::TurnComplete { usage: None });
    if let Err(write_err) = arbos_core::append_events(transcript, &[notice.clone(), close]) {
        crate::klog::error(
            "turn_error_unrecorded",
            Some(id),
            format!("the transcript would not take the notice: {write_err:#}"),
        );
        // Live, at least: the tail will never carry it.
        hooks.broadcast(Frame::Event {
            agent: id.to_string(),
            event: notice,
        });
    }
}

/// A joined task's panic as one line: the message when it was a string,
/// the kind of end otherwise.
fn panic_text(e: tokio::task::JoinError) -> String {
    if e.is_cancelled() {
        return "the turn task was cancelled".into();
    }
    let payload = e.into_panic();
    let text = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with a non-text payload".into());
    arbos_core::text::clip(text.lines().next().unwrap_or("").trim(), 300)
}

/// Kernel hooks scoped to one agent, so live events carry its id.
struct TurnHooks {
    inner: Arc<KernelHooks>,
    agent: AgentId,
}

impl arbos_engine::Hooks for TurnHooks {
    fn claimed(&self, path: &std::path::Path) -> Option<arbos_engine::Claim> {
        self.inner.claim_on(path)
    }

    fn store_read(
        &self,
        address: &str,
    ) -> BoxFuture<'static, anyhow::Result<arbos_engine::StoreFile>> {
        let place = self.inner.place.clone();
        let address = address.to_string();
        Box::pin(async move { crate::hub_link::store_read(&place, &address).await })
    }

    fn store_list(
        &self,
        address: &str,
    ) -> BoxFuture<'static, anyhow::Result<Vec<arbos_core::wire::Entry>>> {
        let place = self.inner.place.clone();
        let address = address.to_string();
        Box::pin(async move { crate::hub_link::store_list(&place, &address).await })
    }

    fn store_write(
        &self,
        address: &str,
        text: String,
        base_hash: Option<String>,
    ) -> BoxFuture<'static, anyhow::Result<arbos_engine::StoreWritten>> {
        let place = self.inner.place.clone();
        let address = address.to_string();
        Box::pin(
            async move { crate::hub_link::store_write(&place, &address, text, base_hash).await },
        )
    }

    fn approve(
        &self,
        agent: &AgentId,
        tool: &str,
        command: &str,
    ) -> BoxFuture<'static, anyhow::Result<bool>> {
        let rx = self.inner.approve(agent, tool, command);
        Box::pin(async move { Ok(rx.await.unwrap_or(false)) })
    }

    fn emit(&self, event: &arbos_core::Event) {
        use arbos_core::{EventKind, wire::Frame};
        self.inner.note_progress(self.agent.as_str());
        // A tool call starting: the kernel's guess at the live line, for
        // an agent that has not said what it is doing this turn.
        if let EventKind::Tool(rec) = &event.kind
            && event.seq == 0
            && rec.ended.is_none()
            && rec.name != "status"
        {
            let step = arbos_core::status::derived(&rec.name, rec.args.as_ref());
            let _ = self.inner.set_status(self.agent.as_str(), &step, "derived");
        }
        // Streamed text goes out as deltas: one frame per chunk, no seq.
        // The whole step follows from the transcript tail as an `event`
        // with its line number, which older clients already render.
        let frame = match &event.kind {
            EventKind::Assistant { text, step, .. } if event.seq == 0 => Frame::AssistantDelta {
                agent: self.agent.to_string(),
                text: text.clone(),
                step: *step,
            },
            EventKind::Thinking { text, step, .. } if event.seq == 0 => Frame::ThinkingDelta {
                agent: self.agent.to_string(),
                text: text.clone(),
                step: *step,
            },
            _ => Frame::Event {
                agent: self.agent.to_string(),
                event: event.clone(),
            },
        };
        self.inner.broadcast(frame);
    }

    /// "status: Running sleep 45" as a one-line reply: the live line takes
    /// the words; the transcript keeps the model's line as it was.
    fn spoke_status(&self, step: &str) {
        self.inner.note_progress(self.agent.as_str());
        let _ = self.inner.set_status(self.agent.as_str(), step, "agent");
    }

    fn kernel_step(&self, step: &str) {
        self.inner.note_progress(self.agent.as_str());
        let _ = self.inner.set_status(self.agent.as_str(), step, "derived");
    }

    fn working(&self, secs: u64) {
        self.inner.broadcast(arbos_core::wire::Frame::Working {
            agent: self.agent.to_string(),
            secs,
        });
    }

    /// One `prompt_size` line per turn in kernel.log: what the first model
    /// call carried before any history, so a grown prompt shows up in
    /// `arbos-kernel log` rather than on the bill.
    fn prompt_size(&self, size: arbos_engine::PromptSize) {
        crate::klog::info(
            "prompt_size",
            Some(self.agent.as_str()),
            format!(
                "model={} system={} tools={} conversation={} total={} (estimated tokens)",
                size.model,
                size.system,
                size.tools,
                size.conversation,
                size.total()
            ),
        );
    }
}
