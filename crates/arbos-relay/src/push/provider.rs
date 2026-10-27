//! The provider seam: one trait every push transport implements, one result type every
//! caller reads. APNs is the only real transport today; a second one (FCM, web push) is a
//! new `impl Provider` plus a `platform` match in [`crate::push::Push::provider_for`].

use std::fmt;

use futures::future::BoxFuture;
use serde_json::Value;
#[cfg(test)]
use tokio::sync::Mutex;

/// One notification as handed to a transport.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub device_token: String,
    /// `development` → sandbox host, anything else → production.
    pub environment: String,
    /// `alert`, `background`, or `voip` (PushKit).
    pub push_type: &'static str,
    /// Overrides the client's topic: VoIP pushes go to `<bundle_id>.voip`.
    pub topic: Option<String>,
    /// APNs `apns-collapse-id`; later notifications with the same id replace earlier ones.
    pub collapse_id: Option<String>,
    pub payload: Value,
}

/// What a transport said about one send. The outbox acts on the variant alone, never on the
/// text: `Accepted` is done, `TokenInvalid` forgets the device token (and every queued push
/// for it), `Transient` retries with backoff until the push expires, `Permanent` drops it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Accepted,
    /// The device token is dead (APNs `410 Unregistered`, `400 BadDeviceToken`, …).
    TokenInvalid {
        reason: String,
    },
    /// Nothing will change by sending the same thing again (bad payload, wrong topic).
    Permanent {
        reason: String,
    },
    /// Worth another try later (transport error, 429, 5xx, provider token hiccup).
    Transient {
        reason: String,
    },
}

impl SendOutcome {
    pub fn token_invalid(reason: impl Into<String>) -> Self {
        Self::TokenInvalid {
            reason: reason.into(),
        }
    }

    pub fn permanent(reason: impl Into<String>) -> Self {
        Self::Permanent {
            reason: reason.into(),
        }
    }

    pub fn transient(reason: impl Into<String>) -> Self {
        Self::Transient {
            reason: reason.into(),
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Accepted => "accepted",
            Self::TokenInvalid { reason }
            | Self::Permanent { reason }
            | Self::Transient { reason } => reason,
        }
    }

    /// One word for logs and the admin view.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::TokenInvalid { .. } => "token_invalid",
            Self::Permanent { .. } => "permanent",
            Self::Transient { .. } => "transient",
        }
    }
}

impl fmt::Display for SendOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accepted => f.write_str("accepted"),
            other => write!(f, "{}: {}", other.label(), other.reason()),
        }
    }
}

/// A push transport. Implementations are cheap to call concurrently and never panic on a
/// bad answer: everything the far side says maps to one [`SendOutcome`].
pub trait Provider: Send + Sync {
    /// Short name for logs (`apns`, `capture`, `off`).
    fn name(&self) -> &'static str;

    /// Can this provider reach a device at all? `false` makes the relay skip the whole
    /// push pipeline for that platform (the in-app `/me/pushes` path still runs).
    fn is_on(&self) -> bool;

    fn send<'a>(&'a self, out: &'a Outgoing) -> BoxFuture<'a, SendOutcome>;
}

/// No key configured: every send is a permanent failure and `is_on` says so first.
pub struct Off;

impl Provider for Off {
    fn name(&self) -> &'static str {
        "off"
    }

    fn is_on(&self) -> bool {
        false
    }

    fn send<'a>(&'a self, _out: &'a Outgoing) -> BoxFuture<'a, SendOutcome> {
        Box::pin(async { SendOutcome::permanent("push is off") })
    }
}

/// Tests: every notification lands in a list instead of at Apple. A script of outcomes can be
/// queued so a test drives retries and dead tokens through the same code the relay runs.
#[cfg(test)]
#[derive(Default)]
pub struct Capture {
    sent: Mutex<Vec<Outgoing>>,
    script: Mutex<Vec<SendOutcome>>,
}

#[cfg(test)]
impl Capture {
    /// Answer the next sends with these outcomes, in order; `Accepted` after the script ends.
    /// Every attempt is still recorded in `captured()`.
    pub async fn script(&self, outcomes: Vec<SendOutcome>) {
        let mut s = self.script.lock().await;
        *s = outcomes;
        s.reverse();
    }

    pub async fn captured(&self) -> Vec<Outgoing> {
        self.sent.lock().await.clone()
    }
}

#[cfg(test)]
impl Provider for Capture {
    fn name(&self) -> &'static str {
        "capture"
    }

    fn is_on(&self) -> bool {
        true
    }

    fn send<'a>(&'a self, out: &'a Outgoing) -> BoxFuture<'a, SendOutcome> {
        Box::pin(async move {
            self.sent.lock().await.push(out.clone());
            self.script
                .lock()
                .await
                .pop()
                .unwrap_or(SendOutcome::Accepted)
        })
    }
}
