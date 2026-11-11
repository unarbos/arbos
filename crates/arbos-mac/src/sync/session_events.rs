//! An account's events keep their original scope through the UI queue.
use super::work_lifetime::{WorkLifetime, WorkTicket};
use super::SyncEvent;

pub(crate) type EventSendResult = Result<(), Box<mpsc::error::SendError<SyncEvent>>>;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Clone)]
pub(crate) struct SessionEvents {
    sender: mpsc::UnboundedSender<SyncEvent>,
    owner: WorkTicket,
    lifetime: Arc<Mutex<WorkLifetime>>,
}

impl SessionEvents {
    pub(crate) fn new(
        sender: mpsc::UnboundedSender<SyncEvent>,
        owner: WorkTicket,
        lifetime: Arc<Mutex<WorkLifetime>>,
    ) -> Self {
        Self {
            sender,
            owner,
            lifetime,
        }
    }

    pub(crate) fn send(&self, event: SyncEvent) -> EventSendResult {
        let Ok(lifetime) = self.lifetime.lock() else {
            return Err(Box::new(mpsc::error::SendError(event)));
        };
        self.send_admitted(&lifetime, event)
    }

    /// Use only while the caller already owns this account's admission fence.
    pub(crate) fn send_admitted(
        &self,
        lifetime: &WorkLifetime,
        event: SyncEvent,
    ) -> EventSendResult {
        if !lifetime.accepts(self.owner) {
            return Err(Box::new(mpsc::error::SendError(event)));
        }
        self.sender
            .send(SyncEvent::ForAccount {
                owner: self.owner,
                event: Box::new(event),
            })
            .map_err(Box::new)
    }
}
