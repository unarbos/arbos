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

    /// Returns false when no turn is running for the agent.
    pub fn steer(&self, id: &str, text: String) -> bool {
        match self.control(id) {
            Some(c) => {
                c.steer(text);
                true
            }
            None => false,
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
        self.in_flight
            .lock()
            .unwrap()
            .insert(id.clone(), control.clone());
        tokio::spawn(async move {
            let agent = match load_agent(&place, &wake.agent) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("load agent: {e:#}");
                    let _ = done.send(id);
                    return;
                }
            };
            if agent.paused {
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
            if let Err(e) = res {
                eprintln!("turn: {e:#}");
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
        // Streamed text goes out as deltas: one frame per chunk, no seq.
        // The whole step follows from the transcript tail as an `event`
        // with its line number, which older clients already render.
        let frame = match &event.kind {
            EventKind::Assistant { text, .. } if event.seq == 0 => Frame::AssistantDelta {
                agent: self.agent.to_string(),
                text: text.clone(),
            },
            EventKind::Thinking { text } if event.seq == 0 => Frame::ThinkingDelta {
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
}
