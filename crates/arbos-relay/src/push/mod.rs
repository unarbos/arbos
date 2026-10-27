//! Push notifications: one provider seam, one durable outbox, one payload policy.
//!
//! ```text
//!  announce / badge_refresh / clear-on-read
//!        │  Push::enqueue(Vec<NewPush>)        ← eligibility already decided by the caller
//!        ▼
//!  push_outbox (SQLite)  ── dedupe by key, retries with backoff, survives restarts
//!        │  outbox worker (Push::spawn_worker)
//!        ▼
//!  Provider::send(Outgoing) → SendOutcome        ← APNs today; FCM/web push = a new impl
//! ```
//!
//! Eligibility (who hears what) is decided before the enqueue, in [`eligibility`]: sender →
//! blocked sender → archived → per-chat level (all / mentions / none / inherit) → large-group
//! mentions-only default. The outbox never second-guesses it.
//!
//! How to add a push kind (the clear-on-read worker's case): add a [`PushKind`] variant with
//! its `push_type`, `collapse_id` and `dedupe_key`, a payload constructor in [`payload`], and
//! enqueue a [`NewPush`] per recipient device. Nothing in the worker or the provider changes.
//!
//! Direct sends ([`Push::send_now`]) bypass the outbox: VoIP rings (must expire at once) and
//! the admin test push. Everything else goes through the outbox.

pub mod eligibility;
pub mod outbox;
pub mod payload;
pub mod provider;
pub mod store;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::db::Db;

pub use payload::PushKind;
pub use provider::{Outgoing, Provider, SendOutcome};
pub use store::NewPush;

/// How long a message alert stays worth sending from the relay's side. (Once APNs accepts
/// it, Apple keeps it for the device for 24 h on its own.)
pub const MESSAGE_TTL_MS: i64 = 15 * 60_000;
/// A badge count older than this is stale anyway (the next read sends a new one).
pub const BADGE_TTL_MS: i64 = 5 * 60_000;

/// The relay's push front door: `state.push`.
pub struct Push {
    pub(crate) db: Db,
    apns: Arc<dyn Provider>,
    #[cfg(test)]
    capture: Option<Arc<provider::Capture>>,
    pub(crate) wake: Notify,
    pub(crate) stats: outbox::Stats,
}

impl Push {
    /// APNs from the environment, or [`provider::Off`] with one log line.
    pub fn from_env(db: Db) -> Self {
        let apns: Arc<dyn Provider> = match crate::apns::from_env() {
            Some(a) => Arc::new(a),
            None => Arc::new(provider::Off),
        };
        Self {
            db,
            apns,
            #[cfg(test)]
            capture: None,
            wake: Notify::new(),
            stats: outbox::Stats::default(),
        }
    }

    /// Tests: every send lands in [`Push::captured`]; nothing leaves the process.
    #[cfg(test)]
    pub fn capture(db: Db) -> Self {
        let cap = Arc::new(provider::Capture::default());
        Self {
            db,
            apns: cap.clone(),
            capture: Some(cap),
            wake: Notify::new(),
            stats: outbox::Stats::default(),
        }
    }

    /// Is any provider able to reach a device? (`false` = no key: skip the pipeline.)
    pub fn is_on(&self) -> bool {
        self.apns.is_on()
    }

    pub fn provider_name(&self) -> &'static str {
        self.apns.name()
    }

    /// The transport for a registered token's `platform`. Only `ios` exists today.
    pub fn provider_for(&self, platform: &str) -> Option<&Arc<dyn Provider>> {
        match platform {
            "ios" => Some(&self.apns),
            _ => None,
        }
    }

    /// Send right now, outside the outbox (VoIP, admin test). No retry, no dedupe.
    pub async fn send_now(&self, out: Outgoing) -> SendOutcome {
        self.apns.send(&out).await
    }

    /// Queue pushes durably and wake the worker. Returns how many were new; the rest were
    /// duplicates of a push already queued or sent (same dedupe key).
    pub fn enqueue(&self, pushes: Vec<NewPush>) -> usize {
        if pushes.is_empty() {
            return 0;
        }
        let wanted = pushes.len();
        match self.db.outbox_enqueue(&pushes) {
            Ok(n) => {
                self.stats.enqueued.fetch_add(n as u64, Ordering::Relaxed);
                self.stats
                    .deduped
                    .fetch_add((wanted - n) as u64, Ordering::Relaxed);
                if n > 0 {
                    self.wake.notify_one();
                }
                n
            }
            Err(e) => {
                tracing::error!(error = %e, pushes = wanted, "push outbox: enqueue failed");
                0
            }
        }
    }

    /// Start the background worker (once, at relay start).
    pub fn spawn_worker(self: &Arc<Self>) {
        outbox::spawn(self.clone());
    }

    /// Work the queue until nothing is due (tests, and `POST /admin/push/drain`). Returns how
    /// many rows were handled.
    pub async fn drain(&self) -> usize {
        let mut total = 0;
        for _ in 0..64 {
            let n = outbox::run_pass(self).await;
            if n == 0 {
                break;
            }
            total += n;
        }
        total
    }

    /// Everything the capture provider was asked to send.
    #[cfg(test)]
    pub async fn captured(&self) -> Vec<Outgoing> {
        match &self.capture {
            Some(c) => c.captured().await,
            None => Vec::new(),
        }
    }

    /// Tests: the capture provider answers the next sends with these outcomes.
    #[cfg(test)]
    pub async fn script(&self, outcomes: Vec<SendOutcome>) {
        if let Some(c) = &self.capture {
            c.script(outcomes).await;
        }
    }

    /// Admin view: provider, counters since start, and what the table holds.
    pub fn status(&self) -> Value {
        let counts = self.db.outbox_counts().unwrap_or_default();
        json!({
            "provider": self.provider_name(),
            "on": self.is_on(),
            "outbox": {
                "queued": counts.queued,
                "due": counts.due,
                "in_flight": counts.in_flight,
                "sent_24h": counts.sent_recent,
            },
            "since_start": self.stats.json(),
            "retry": { "base_ms": outbox::RETRY_BASE_MS, "max_ms": outbox::RETRY_MAX_MS, "message_ttl_ms": MESSAGE_TTL_MS },
        })
    }
}

#[cfg(test)]
mod tests {
    //! The outbox contract: durable across a restart, one push per device per message,
    //! retries with backoff, dead tokens forgotten.

    use super::*;
    use crate::db_push::PushToken;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    use arbos_core::ids::{now_ms, ulid};

    fn token_for(state: &crate::state::AppState, device: &str, user: &str) -> String {
        state
            .db
            .insert_device(device, "phone", &ulid(), user, "github")
            .expect("device");
        let tok = format!(
            "{:0<64}",
            ulid()
                .to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .collect::<String>()
        );
        state
            .db
            .upsert_push_token(&PushToken {
                device_id: device.to_owned(),
                user_id: user.to_owned(),
                platform: "ios".into(),
                token: tok.clone(),
                environment: "development".into(),
                bundle_id: crate::apns::DEFAULT_TOPIC.into(),
                app_version: "1".into(),
                updated_at: now_ms(),
            })
            .expect("token");
        tok
    }

    fn message(user: &str, device: &str, chat: &str, msg: &str) -> NewPush {
        let kind = PushKind::Message {
            chat_id: chat.to_owned(),
            message_id: msg.to_owned(),
        };
        NewPush {
            user_id: user.to_owned(),
            device_id: device.to_owned(),
            collapse_id: Some(kind.collapse_id(user)),
            kind,
            payload: json!({ "aps": { "alert": { "title": "t", "body": msg } } }),
            ttl_ms: MESSAGE_TTL_MS,
        }
    }

    #[tokio::test]
    async fn a_message_pushes_at_most_once_per_device() {
        let state = test_state(AuthMode::Github);
        let user = ulid();
        let tok = token_for(&state, "dev-a", &user);
        let tok_b = token_for(&state, "dev-b", &user);
        // The same message announced twice (two notify calls, a retry from the box…).
        assert_eq!(
            state.push.enqueue(vec![
                message(&user, "dev-a", "chat", "m1"),
                message(&user, "dev-b", "chat", "m1"),
            ]),
            2
        );
        assert_eq!(
            state
                .push
                .enqueue(vec![message(&user, "dev-a", "chat", "m1")]),
            0,
            "the duplicate is ignored while queued"
        );
        assert_eq!(state.push.drain().await, 2);
        assert_eq!(
            state
                .push
                .enqueue(vec![message(&user, "dev-a", "chat", "m1")]),
            0,
            "and after it was sent"
        );
        assert_eq!(state.push.drain().await, 0);
        let sent = state.push.captured().await;
        assert_eq!(sent.len(), 2);
        let mut tokens: Vec<&str> = sent.iter().map(|o| o.device_token.as_str()).collect();
        tokens.sort();
        let mut want = vec![tok.as_str(), tok_b.as_str()];
        want.sort();
        assert_eq!(tokens, want);
        assert_eq!(sent[0].collapse_id.as_deref(), Some("msg:m1"));
        assert_eq!(sent[0].push_type, "alert");
        // A different message is its own push.
        assert_eq!(
            state
                .push
                .enqueue(vec![message(&user, "dev-a", "chat", "m2")]),
            1
        );
        assert_eq!(state.push.drain().await, 1);
        let s = state.push.status();
        assert_eq!(s["since_start"]["delivered"], 3);
        assert_eq!(s["since_start"]["deduped"], 2);
        assert_eq!(s["outbox"]["queued"], 0);
        assert_eq!(s["outbox"]["sent_24h"], 3);
    }

    #[tokio::test]
    async fn queued_pushes_survive_a_relay_restart() {
        let state = test_state(AuthMode::Github);
        let user = ulid();
        let tok = token_for(&state, "dev-a", &user);
        // Enqueued, then the process dies before the worker ran (or mid-send: the lease is
        // what a crashed worker leaves behind).
        state.push.enqueue(vec![
            message(&user, "dev-a", "chat", "m1"),
            message(&user, "dev-a", "chat", "m2"),
        ]);
        let leased = state
            .db
            .outbox_claim_due(1, outbox::LEASE_MS)
            .expect("claim");
        assert_eq!(
            leased.len(),
            1,
            "one row is in flight when the process dies"
        );
        assert!(state.push.captured().await.is_empty());
        let path = state.cfg.data_dir.join("relay.db");
        drop(state);

        // A new process on the same database.
        let db = Db::open(&path).expect("reopen");
        let push = Arc::new(Push::capture(db.clone()));
        assert_eq!(db.outbox_counts().expect("counts").queued, 2);
        assert_eq!(db.outbox_release_leases().expect("leases"), 1);
        assert_eq!(push.drain().await, 2, "both go out after the restart");
        let sent = push.captured().await;
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|o| o.device_token == tok));
        let mut bodies: Vec<String> = sent
            .iter()
            .map(|o| {
                o.payload["aps"]["alert"]["body"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        bodies.sort();
        assert_eq!(bodies, vec!["m1", "m2"]);
        assert_eq!(db.outbox_counts().expect("counts").queued, 0);
        // The spawned worker lifts leases itself at start (same call), so a real relay needs
        // no operator step.
        push.spawn_worker();
    }

    #[tokio::test]
    async fn transient_failures_retry_with_backoff_then_deliver() {
        let state = test_state(AuthMode::Github);
        let user = ulid();
        token_for(&state, "dev-a", &user);
        state
            .push
            .script(vec![
                SendOutcome::transient("apns 503: Shutdown"),
                SendOutcome::transient("apns transport: reset"),
            ])
            .await;
        state
            .push
            .enqueue(vec![message(&user, "dev-a", "chat", "m1")]);
        assert_eq!(state.push.drain().await, 1);
        let queued = state.db.outbox_queued().expect("queued");
        assert_eq!(queued.len(), 1, "still queued after a transient failure");
        assert_eq!(queued[0].attempts, 1);
        let before = now_ms();
        let next = state.db.outbox_next_due().expect("due").expect("a row");
        assert!(next > before, "not due at once: backoff of ~1 s");
        assert!(next - before <= 1_300);
        assert_eq!(state.push.drain().await, 0, "nothing due yet");

        tokio::time::sleep(std::time::Duration::from_millis(
            (next - now_ms()).max(0) as u64 + 20,
        ))
        .await;
        assert_eq!(state.push.drain().await, 1, "second attempt (fails again)");
        let queued = state.db.outbox_queued().expect("queued");
        assert_eq!(queued[0].attempts, 2);
        let next2 = state.db.outbox_next_due().expect("due").expect("a row");
        assert!(next2 - now_ms() > 1_300, "the second wait is longer: ~2 s");
        state.db.outbox_force_due().expect("force");
        assert_eq!(state.push.drain().await, 1, "third attempt is accepted");
        assert!(state.db.outbox_queued().expect("queued").is_empty());
        assert_eq!(
            state.push.captured().await.len(),
            3,
            "three attempts, one push"
        );
        let s = state.push.status();
        assert_eq!(s["since_start"]["retried"], 2);
        assert_eq!(s["since_start"]["delivered"], 1);
    }

    #[tokio::test]
    async fn a_dead_token_is_forgotten_with_its_queue_and_a_permanent_error_drops_one() {
        let state = test_state(AuthMode::Github);
        let user = ulid();
        let tok_a = token_for(&state, "dev-a", &user);
        let tok_b = token_for(&state, "dev-b", &user);
        state
            .push
            .script(vec![SendOutcome::token_invalid("apns 410: Unregistered")])
            .await;
        // Two messages for the dead device, one for the live one. dev-a's second push is
        // waiting on a retry timer, so it is not in this pass; the 410 for the first one
        // must still take it out.
        state.push.enqueue(vec![
            message(&user, "dev-a", "chat", "m1"),
            message(&user, "dev-a", "chat", "m2"),
            message(&user, "dev-b", "chat", "m2"),
        ]);
        let waiting = state
            .db
            .outbox_queued()
            .expect("queued")
            .into_iter()
            .find(|r| {
                r.device_id == "dev-a"
                    && matches!(&r.kind, PushKind::Message { message_id, .. } if message_id == "m2")
            })
            .expect("dev-a m2");
        state
            .db
            .outbox_retry_later(waiting.id, now_ms() + 60_000, "later")
            .expect("defer");
        assert_eq!(
            state.push.drain().await,
            2,
            "m1 to dev-a (410) and m2 to dev-b"
        );
        let tokens = state.db.push_tokens_of_user(&user).expect("tokens");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].token, tok_b);
        assert!(tokens.iter().all(|t| t.token != tok_a));
        let sent = state.push.captured().await;
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().any(|o| o.device_token == tok_a));
        assert!(sent.iter().any(|o| o.device_token == tok_b));
        assert!(
            state.db.outbox_queued().expect("queued").is_empty(),
            "dev-a's deferred push went with its token"
        );

        state
            .push
            .script(vec![SendOutcome::permanent("apns 400: PayloadTooLarge")])
            .await;
        state
            .push
            .enqueue(vec![message(&user, "dev-b", "chat", "m3")]);
        assert_eq!(state.push.drain().await, 1);
        assert!(
            state.db.outbox_queued().expect("queued").is_empty(),
            "dropped, not retried"
        );
        let s = state.push.status();
        assert_eq!(s["since_start"]["tokens_forgotten"], 1);
        assert_eq!(s["since_start"]["dropped_permanent"], 1);
        assert_eq!(s["since_start"]["delivered"], 1);
    }

    #[tokio::test]
    async fn a_signed_out_device_gets_nothing_and_an_expired_push_is_dropped() {
        let state = test_state(AuthMode::Github);
        let user = ulid();
        token_for(&state, "dev-a", &user);
        state
            .push
            .enqueue(vec![message(&user, "dev-a", "chat", "m1")]);
        state.db.delete_push_token("dev-a").expect("sign out");
        assert_eq!(state.push.drain().await, 1);
        assert!(state.push.captured().await.is_empty());
        assert_eq!(state.push.status()["since_start"]["dropped_no_token"], 1);

        token_for(&state, "dev-b", &user);
        let mut p = message(&user, "dev-b", "chat", "m2");
        p.ttl_ms = 1_000;
        state.push.enqueue(vec![p]);
        state.db.outbox_expire_all().expect("expire");
        assert_eq!(state.push.drain().await, 0);
        assert!(state.push.captured().await.is_empty());
        assert_eq!(state.push.status()["since_start"]["dropped_expired"], 1);
        assert!(state.db.outbox_queued().expect("queued").is_empty());
    }

    #[test]
    fn apns_answers_map_to_the_four_outcomes() {
        use crate::apns::classify;
        assert!(matches!(
            classify(410, "Unregistered"),
            SendOutcome::TokenInvalid { .. }
        ));
        assert!(matches!(
            classify(400, "BadDeviceToken"),
            SendOutcome::TokenInvalid { .. }
        ));
        assert!(matches!(
            classify(400, "DeviceTokenNotForTopic"),
            SendOutcome::TokenInvalid { .. }
        ));
        assert!(matches!(
            classify(429, "TooManyRequests"),
            SendOutcome::Transient { .. }
        ));
        assert!(matches!(
            classify(503, "Shutdown"),
            SendOutcome::Transient { .. }
        ));
        assert!(matches!(
            classify(500, "InternalServerError"),
            SendOutcome::Transient { .. }
        ));
        assert!(matches!(
            classify(403, "InvalidProviderToken"),
            SendOutcome::Transient { .. }
        ));
        assert!(matches!(
            classify(400, "BadCollapseId"),
            SendOutcome::Permanent { .. }
        ));
        assert!(matches!(
            classify(413, "PayloadTooLarge"),
            SendOutcome::Permanent { .. }
        ));
        assert!(matches!(
            classify(400, "TopicDisallowed"),
            SendOutcome::Permanent { .. }
        ));
    }
}
