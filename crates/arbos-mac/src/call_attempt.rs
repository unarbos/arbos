//! One local start/join lease shared by REST, UI and media ownership.
use crate::sync::work_lifetime::WorkTicket;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone, Debug)]
pub struct CallAttempt(Arc<Attempt>);

#[derive(Debug)]
struct Attempt {
    cancelled: watch::Sender<bool>,
    account: WorkTicket,
    backend: WorkTicket,
    chat_id: String,
    call_id: Option<String>,
}

impl CallAttempt {
    pub(crate) fn new(
        chat_id: String,
        call_id: Option<String>,
        account: WorkTicket,
        backend: WorkTicket,
    ) -> Self {
        Self(Arc::new(Attempt {
            cancelled: watch::channel(false).0,
            account,
            backend,
            chat_id,
            call_id,
        }))
    }

    pub(crate) fn chat_id(&self) -> &str {
        &self.0.chat_id
    }

    pub(crate) fn backend(&self) -> WorkTicket {
        self.0.backend
    }

    pub(crate) fn account(&self) -> WorkTicket {
        self.0.account
    }

    pub fn is_active(&self) -> bool {
        !*self.0.cancelled.borrow()
    }

    pub fn cancel(&self) {
        self.0.cancelled.send_replace(true);
    }

    pub fn is_same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn owns(&self, other: &Self) -> bool {
        self.is_active() && self.is_same(other)
    }

    /// Old cleanup must not remove membership shared with the replacement.
    pub fn protects(&self, call_id: &str, chat_id: &str) -> bool {
        self.is_active()
            && (self.0.call_id.as_deref() == Some(call_id) || self.0.chat_id == chat_id)
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.0.cancelled.subscribe();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}
