//! Audit rows for group / server / channel settings changes (P0 2026-10-27: members of
//! "Arbonites" renamed the group and swapped its picture). Every attempt — allowed or denied —
//! leaves one row in the `audit` table under the entity's blob slot (`people:{chat}`,
//! `servers:{server}`), kind `settings`, so `who changed what, when, and was it allowed` is
//! answerable without a request log.

use serde_json::json;

use arbos_core::ids::now_ms;

use crate::auth::Principal;
use crate::state::AppState;

pub const KIND: &str = "settings";

/// What a principal looks like in an audit row: `user:<id>` with its device, `box:<owner>`
/// for an agent's box, `admin` for the operator token.
fn who(principal: &Principal) -> (String, Option<String>) {
    match principal {
        Principal::Device { id, user_id, .. } => (format!("user:{user_id}"), Some(id.clone())),
        Principal::Box { user_id } => (format!("box:{user_id}"), None),
        Principal::Admin => ("admin".to_owned(), None),
    }
}

/// One row per attempt. `slot` is the entity's blob slot; `target` names the entity
/// (`chat`, `server`, `channel`); `role` is the caller's role in it when known.
#[allow(clippy::too_many_arguments)]
pub fn record(
    state: &AppState,
    slot: &str,
    target: &str,
    target_id: &str,
    principal: &Principal,
    role: Option<&str>,
    fields: &[&str],
    allowed: bool,
    reason: &str,
) {
    let (principal_label, device_id) = who(principal);
    let entry = json!({
        "ts": now_ms(),
        "target": target,
        "id": target_id,
        "principal": principal_label,
        "device_id": device_id,
        "role": role,
        "fields": fields,
        "allowed": allowed,
        "reason": reason,
    });
    if let Err(e) = state.db.record_audit(slot, KIND, &entry) {
        tracing::warn!(slot, error = %e, "settings audit row not written");
    }
    tracing::info!(
        slot,
        principal = %principal_label,
        ?role,
        ?fields,
        allowed,
        reason,
        "settings change attempt"
    );
}
