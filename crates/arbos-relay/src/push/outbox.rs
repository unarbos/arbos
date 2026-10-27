//! The outbox worker: claims due rows from [`super::store`], resolves the device's current
//! token, sends through the provider for that platform and acts on the [`SendOutcome`].
//!
//! Retries: exponential backoff from [`RETRY_BASE_MS`] doubling per attempt up to
//! [`RETRY_MAX_MS`], with ±25 % jitter so a burst of failures does not come back as one
//! burst; a push whose next attempt would fall past its `expires_at` is dropped instead.
//! Dead tokens (`TokenInvalid`) delete the token and every queued push for that device.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use rand::Rng;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;

use super::provider::{Outgoing, SendOutcome};
use super::store::OutboxRow;
use super::Push;

/// Rows taken per pass.
pub const BATCH: usize = 64;
/// Sends in flight at once.
pub const CONCURRENCY: usize = 16;
/// A claimed row that is neither sent nor released within this time is claimed again (the
/// process died mid-send).
pub const LEASE_MS: i64 = 60_000;
pub const RETRY_BASE_MS: i64 = 1_000;
pub const RETRY_MAX_MS: i64 = 60_000;
/// Longest the worker sleeps with nothing due (a wake-up cuts it short).
const IDLE_MAX: Duration = Duration::from_secs(30);

/// Counters since the process started (the admin view; the durable truth is the table).
#[derive(Default)]
pub struct Stats {
    pub enqueued: AtomicU64,
    pub deduped: AtomicU64,
    pub delivered: AtomicU64,
    pub retried: AtomicU64,
    pub dropped_expired: AtomicU64,
    pub dropped_permanent: AtomicU64,
    pub dropped_no_token: AtomicU64,
    pub tokens_forgotten: AtomicU64,
}

impl Stats {
    fn bump(&self, c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    pub fn json(&self) -> Value {
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        json!({
            "enqueued": g(&self.enqueued),
            "deduped": g(&self.deduped),
            "delivered": g(&self.delivered),
            "retried": g(&self.retried),
            "dropped_expired": g(&self.dropped_expired),
            "dropped_permanent": g(&self.dropped_permanent),
            "dropped_no_token": g(&self.dropped_no_token),
            "tokens_forgotten": g(&self.tokens_forgotten),
        })
    }
}

/// When to try again after `attempts` failed tries (the next one is attempt `attempts + 1`).
pub fn backoff_ms(attempts: i64) -> i64 {
    let exp = attempts.clamp(0, 16) as u32;
    let base = (RETRY_BASE_MS << exp).min(RETRY_MAX_MS);
    let jitter = base / 4;
    let delta = if jitter > 0 {
        rand::thread_rng().gen_range(-jitter..=jitter)
    } else {
        0
    };
    (base + delta).max(RETRY_BASE_MS / 2)
}

/// The background loop: runs for the life of the process. Leases left by a previous process
/// are lifted first, so a push that was in flight at a crash goes out again now.
pub fn spawn(push: Arc<Push>) {
    tokio::spawn(async move {
        match push.db.outbox_release_leases() {
            Ok(n) if n > 0 => tracing::info!(rows = n, "push outbox: resuming in-flight pushes"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "push outbox: releasing leases"),
        }
        if let Ok(counts) = push.db.outbox_counts() {
            if counts.queued > 0 {
                tracing::info!(
                    queued = counts.queued,
                    "push outbox: queued pushes survive the restart"
                );
            }
        }
        loop {
            let processed = run_pass(&push).await;
            if processed > 0 {
                continue;
            }
            let wait = match push.db.outbox_next_due() {
                Ok(Some(at)) => Duration::from_millis(
                    (at - now_ms()).clamp(50, IDLE_MAX.as_millis() as i64) as u64,
                ),
                _ => IDLE_MAX,
            };
            tokio::select! {
                _ = push.wake.notified() => {}
                _ = tokio::time::sleep(wait) => {}
            }
        }
    });
}

/// One pass: expired rows out, due rows claimed and sent. Returns how many rows were handled
/// (0 = nothing due right now).
pub async fn run_pass(push: &Push) -> usize {
    match push.db.outbox_drop_expired() {
        Ok(n) if n > 0 => {
            for _ in 0..n {
                push.stats.bump(&push.stats.dropped_expired);
            }
            tracing::warn!(rows = n, "push outbox: dropped expired pushes unsent");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "push outbox: expiry sweep"),
    }
    let rows = match push.db.outbox_claim_due(BATCH, LEASE_MS) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "push outbox: claiming due rows");
            return 0;
        }
    };
    let n = rows.len();
    futures::stream::iter(rows)
        .for_each_concurrent(CONCURRENCY, |row| async move { handle(push, row).await })
        .await;
    n
}

async fn handle(push: &Push, row: OutboxRow) {
    let kind = row.kind.label();
    let Some(token) = push
        .db
        .push_tokens_of_user(&row.user_id)
        .unwrap_or_default()
        .into_iter()
        .find(|t| t.device_id == row.device_id)
    else {
        // Signed out, token forgotten, or the device is revoked: nothing to deliver to.
        push.stats.bump(&push.stats.dropped_no_token);
        let _ = push.db.outbox_drop(row.id);
        tracing::info!(device = %row.device_id, kind, "push outbox: no live token, dropped");
        return;
    };
    let Some(provider) = push.provider_for(&token.platform) else {
        push.stats.bump(&push.stats.dropped_permanent);
        let _ = push.db.outbox_drop(row.id);
        tracing::warn!(device = %row.device_id, platform = %token.platform, kind, "push outbox: no provider for platform, dropped");
        return;
    };
    let out = Outgoing {
        device_token: token.token.clone(),
        environment: token.environment.clone(),
        push_type: row.kind.push_type(),
        topic: None,
        collapse_id: row.collapse_id.clone(),
        payload: row.payload.clone(),
    };
    let outcome = provider.send(&out).await;
    match outcome {
        SendOutcome::Accepted => {
            push.stats.bump(&push.stats.delivered);
            if let Err(e) = push.db.outbox_mark_sent(row.id) {
                tracing::warn!(error = %e, id = row.id, "push outbox: marking sent");
            }
        }
        SendOutcome::TokenInvalid { reason } => {
            push.stats.bump(&push.stats.tokens_forgotten);
            let _ = push.db.delete_push_token_value(&token.token);
            let dropped = push.db.outbox_drop_device(&row.device_id).unwrap_or(0);
            tracing::info!(
                device = %row.device_id, user = %row.user_id, reason = %reason, dropped,
                "push outbox: token is dead; forgotten with its queued pushes"
            );
        }
        SendOutcome::Permanent { reason } => {
            push.stats.bump(&push.stats.dropped_permanent);
            let _ = push.db.outbox_drop(row.id);
            tracing::warn!(
                device = %row.device_id, kind, attempts = row.attempts + 1, reason = %reason,
                "push outbox: permanent failure, dropped"
            );
        }
        SendOutcome::Transient { reason } => {
            let next = now_ms() + backoff_ms(row.attempts);
            if next >= row.expires_at {
                push.stats.bump(&push.stats.dropped_expired);
                let _ = push.db.outbox_drop(row.id);
                tracing::warn!(
                    device = %row.device_id, kind, attempts = row.attempts + 1, reason = %reason,
                    "push outbox: gave up (expired) after transient failures"
                );
            } else {
                push.stats.bump(&push.stats.retried);
                let _ = push.db.outbox_retry_later(row.id, next, &reason);
                tracing::info!(
                    device = %row.device_id, kind, attempts = row.attempts + 1, reason = %reason,
                    retry_in_ms = next - now_ms(), "push outbox: transient failure, will retry"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_with_jitter_and_caps() {
        for _ in 0..50 {
            let b0 = backoff_ms(0);
            assert!((750..=1_250).contains(&b0), "{b0}");
            let b3 = backoff_ms(3);
            assert!((6_000..=10_000).contains(&b3), "{b3}");
            let b20 = backoff_ms(20);
            assert!((45_000..=75_000).contains(&b20), "{b20}");
        }
    }
}
