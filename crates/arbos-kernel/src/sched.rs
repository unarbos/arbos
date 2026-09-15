use arbos_core::{AgentId, Place, Wake, files::load_agent};
use arbos_engine::{BoxFuture, Host, Registry, TurnControl, turn};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

use crate::hooks::KernelHooks;

pub const MAX_CHILDREN: usize = 8;
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
            let agent = match load_agent(&place, &wake.agent).map(|mut a| {
                arbos_core::project::apply_role(&place, &mut a);
                a
            }) {
                Ok(a) => a,
                Err(e) => {
                    crate::klog::error("load_agent", Some(&id), format!("{e:#}"));
                    let _ = done.send(id);
                    return;
                }
            };
            if agent.paused {
                let _ = done.send(id);
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
                let _ = done.send(id);
                return;
            }
            let wrap = TurnHooks {
                inner: hooks,
                agent: wake.agent.clone(),
            };
            let res = turn(arbos_engine::TurnOpts {
                place,
                agent,
                wake,
                host,
                registry,
                grep,
                hooks: Arc::new(wrap),
                control,
            })
            .await;
            match &res {
                Ok(()) => crate::klog::info(
                    "turn_end",
                    Some(&id),
                    format!("{:.1}s", started.elapsed().as_secs_f64()),
                ),
                Err(e) => crate::klog::error(
                    "turn_error",
                    Some(&id),
                    format!("{e:#} after {:.1}s", started.elapsed().as_secs_f64()),
                ),
            }
            let _ = done.send(id);
        });
    }
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Kernel hooks scoped to one agent, so live events carry its id.
struct TurnHooks {
    inner: Arc<KernelHooks>,
    agent: AgentId,
}

impl arbos_engine::Hooks for TurnHooks {
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
            EventKind::Assistant { text, .. } if event.seq == 0 => Frame::AssistantDelta {
                agent: self.agent.to_string(),
                text: text.clone(),
            },
            EventKind::Thinking { text, .. } if event.seq == 0 => Frame::ThinkingDelta {
                agent: self.agent.to_string(),
                text: text.clone(),
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
        let _ = self.inner.set_status(self.agent.as_str(), step, "agent");
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
