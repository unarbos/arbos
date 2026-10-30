//! Box upgrade: recreate a user's Daytona box from the current default snapshot and carry `/work`
//! over. agentd is baked into the image, so a box made from an older snapshot never gets new
//! agent features until it is upgraded.
//!
//! Steps (one task per user, idempotent while it runs):
//!   measure — start the old sandbox if needed and `du /work` while agentd keeps running; over the
//!             carry limit (or no spool room on the relay) → refused: nothing stopped, box state
//!             untouched, `upgrade.last_refused` says why (ARB-339; before, the box's agentd was
//!             killed first and an oversize box was restarted on every attempt);
//!   pack    — stop its agentd, list `/work` (logs excluded), carry it in parts through the relay's
//!             spool disk (`upgrade_carry`: per-part tar → sha256 → download with a size-scaled
//!             deadline and retries); a failure here revives the old box in place;
//!   stop    — hibernate the old sandbox and record it in `box_history` (kept 7 days as fallback; the 2
//!             newest per user stay unarchived, older retained ones are Daytona-archived at once);
//!   create  — a fresh box from the target snapshot, staged: new tokens, the `boxes` row still
//!             names the source;
//!   unpack  — stop the new agentd, check room (bytes and inodes), upload / verify / extract part by
//!             part, count the tree against the plan, reset the per-workspace owner markers;
//!   start   — the swap: the row now names the new box, which is bounced so the entrypoint starts
//!             agentd on the carried `/work` and must say hello.
//! Progress is in `GET /machine` (`upgrade.phase`, e.g. `pack 3/16`). Any failure after pack rolls
//! back (see [`rollback`]): the row is back on the source first, then the new box is deleted when it
//! holds nothing of `/work`, else kept stopped in `box_history` for the sweeper.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use arbos_core::ids::now_ms;
use arbos_core::wire::BoxState;

use crate::auth::Principal;
use crate::boxes::{self, Provider};
use crate::daytona::SandboxState;
use crate::signin::api_error;
use crate::state::{AppState, UpgradeRefusal, UpgradeStatus};
use crate::upgrade_carry::{self as carry, CarryConfig, Verdict};

const KEEP_OLD_SANDBOX_MS: i64 = 7 * 24 * 3_600_000;
/// Retained sandboxes a user keeps *unarchived* (started in seconds for rollback / repair); older ones
/// are Daytona-archived right after the next upgrade — still within the 7-day window, never deleted
/// early, just off the disk quota (2026-10-22: 308 retained 10 GiB sandboxes filled the 5000 GiB quota,
/// `daytona create` answered 400 and four real users' boxes went to `error`).
const KEEP_UNARCHIVED_RETAINED: usize = 2;

/// Phrase every refusal carries: the deploy worker's skip rule (#1941) matches on it.
const REFUSED_PHRASE: &str = "too large to carry";

/// Is the box behind the relay's default snapshot?
pub fn image_outdated(state: &AppState, sb: &crate::db::Sandbox) -> bool {
    (sb.provider == "daytona" || sb.provider == "gvisor")
        && sb
            .snapshot
            .as_deref()
            .is_some_and(|s| *s != state.default_snapshot())
}

/// `upgrade` object for `GET /machine` and `/admin/users`. `last_refused` (reason, at, work_mib)
/// is present while the newest attempt was refused at the measure step: the box is fine and
/// still on its old image; the deploy worker reads it to skip the box instead of waiting.
pub async fn status_json(state: &AppState, user_id: &str) -> Value {
    let running = state.upgrading.lock().await.get(user_id).cloned();
    let mut v = match running {
        Some(u) => json!({
            "in_progress": true, "phase": u.phase, "since": u.since,
            "from": u.from_snapshot, "to": u.to_snapshot,
        }),
        None => json!({ "in_progress": false }),
    };
    if let Some(r) = state.upgrade_refused.lock().await.get(user_id) {
        v["last_refused"] = json!({ "reason": r.reason, "at": r.at, "work_mib": r.work_mib });
    }
    v
}

/// Record the phase for `GET /machine` and mark the box as waking (it is being worked on).
async fn set_phase(state: &AppState, user_id: &str, phase: &str) {
    note_phase(state, user_id, phase).await;
    state
        .set_box_state(
            user_id,
            BoxState::Waking,
            Some(&format!("upgrading: {phase}")),
        )
        .await;
}

/// Record the phase only — the box keeps serving (measure step).
async fn note_phase(state: &AppState, user_id: &str, phase: &str) {
    if let Some(u) = state.upgrading.lock().await.get_mut(user_id) {
        u.phase = phase.to_owned();
    }
}

/// The measure step said no: remember why, log the line the deploy worker greps, leave the box
/// exactly as it was.
async fn record_refusal(state: &AppState, user_id: &str, reason: &str, work_mib: i64) {
    debug_assert!(reason.contains(REFUSED_PHRASE));
    tracing::error!(user = %user_id, error = %reason, "box upgrade failed");
    tracing::warn!(user = %user_id, work_mib, reason, "box upgrade refused before quiescing; the box keeps running on its image");
    state.upgrade_refused.lock().await.insert(
        user_id.to_owned(),
        UpgradeRefusal {
            reason: reason.to_owned(),
            at: now_ms(),
            work_mib,
        },
    );
    let _ = state.db.record_audit(
        "relay-admin",
        "box_upgrade_refused",
        &json!({ "ts": now_ms(), "user_id": user_id, "reason": reason, "work_mib": work_mib }),
    );
}

/// A planned restart is under way (`/admin/restart`, SIGTERM): no new run may start, or the
/// restart would cut it short with the old box stopped and the new one half made (#2262). Called
/// under the `upgrading` lock, which the restart holds while it checks for runs in flight.
fn refuse_while_restarting(state: &AppState) -> Result<()> {
    if state.restart.box_work_frozen() {
        return Err(anyhow!(
            "{RESTARTING_PHRASE}; the box was not touched, try again after the restart"
        ));
    }
    Ok(())
}

/// Words the deploy worker can match in a refused upgrade.
pub const RESTARTING_PHRASE: &str = "the relay is restarting";

/// What `run` ended with.
#[derive(Debug)]
enum Outcome {
    /// Carried; the report goes to the audit log.
    Done(Value),
    /// Refused at the measure step; nothing changed on the box.
    Refused { reason: String, work_mib: i64 },
}

/// Start an upgrade for `user_id` in the background. `Ok(false)` when one is already running
/// or the box is already on the target snapshot.
pub async fn start(state: &AppState, user_id: &str, force: bool) -> Result<bool> {
    let sb = state
        .db
        .user_box(user_id)?
        .ok_or_else(|| anyhow!("no box for user {user_id}"))?;
    let provider = Provider::parse(&sb.provider);
    if provider == Some(Provider::Gvisor) {
        return start_gvisor(state, user_id, &sb, force).await;
    }
    if provider != Some(Provider::Daytona) {
        return Err(anyhow!(
            "only Daytona and gvisor boxes can be upgraded (this one is {})",
            sb.provider
        ));
    }
    if sb.sandbox_id.is_none() {
        return Err(anyhow!(
            "the box row has no sandbox; use POST /admin/users/{{id}}/box/repair to re-adopt the retained one"
        ));
    }
    let target = state
        .db
        .user_snapshot(user_id)?
        .unwrap_or_else(|| state.default_snapshot());
    if !force && sb.snapshot.as_deref() == Some(target.as_str()) {
        return Ok(false);
    }
    {
        let mut running = state.upgrading.lock().await;
        if running.contains_key(user_id) {
            return Ok(false);
        }
        refuse_while_restarting(state)?;
        running.insert(
            user_id.to_owned(),
            UpgradeStatus {
                phase: "queued".into(),
                since: now_ms(),
                from_snapshot: sb.snapshot.clone(),
                to_snapshot: target.clone(),
            },
        );
    }
    let state2 = state.clone();
    let user = user_id.to_owned();
    tokio::spawn(async move {
        let t0 = Instant::now();
        let outcome = run(&state2, &user, &target, Source::CurrentBox).await;
        state2.upgrading.lock().await.remove(&user);
        finish(&state2, &user, outcome, Task::Upgrade, t0).await;
    });
    Ok(true)
}

/// Which background task is ending — the two share `run` and `finish`.
#[derive(Debug, Clone, Copy)]
enum Task {
    Upgrade,
    Repair,
}

impl Task {
    fn done(self) -> &'static str {
        match self {
            Task::Upgrade => "box upgraded",
            Task::Repair => "box repaired",
        }
    }
    /// The deploy worker greps `box upgrade failed user=<uid> error=…` — keep the words.
    fn failed(self) -> &'static str {
        match self {
            Task::Upgrade => "box upgrade failed",
            Task::Repair => "box repair failed",
        }
    }
    fn audit(self) -> &'static str {
        match self {
            Task::Upgrade => "box_upgrade",
            Task::Repair => "box_repair",
        }
    }
    fn audit_failed(self) -> &'static str {
        match self {
            Task::Upgrade => "box_upgrade_failed",
            Task::Repair => "box_repair_failed",
        }
    }
    fn detail(self) -> &'static str {
        match self {
            Task::Upgrade => "upgrade failed",
            Task::Repair => "repair failed",
        }
    }
}

/// Log, audit and set the box state for the end of an upgrade or repair task.
async fn finish(state: &AppState, user: &str, outcome: Result<Outcome>, task: Task, t0: Instant) {
    let ms = t0.elapsed().as_millis() as i64;
    match outcome {
        Ok(Outcome::Done(report)) => {
            tracing::info!(user = %user, ?report, ms, "{}", task.done());
            state.upgrade_refused.lock().await.remove(user);
            let _ = state.db.record_audit(
                "relay-admin",
                task.audit(),
                &json!({ "ts": now_ms(), "user_id": user, "report": report, "ms": ms }),
            );
        }
        Ok(Outcome::Refused { reason, work_mib }) => {
            record_refusal(state, user, &reason, work_mib).await;
        }
        Err(e) => {
            tracing::error!(user = %user, error = %format!("{e:#}"), "{}", task.failed());
            let _ = state.db.record_audit(
                "relay-admin",
                task.audit_failed(),
                &json!({ "ts": now_ms(), "user_id": user, "error": format!("{e:#}") }),
            );
            state
                .set_box_state(
                    user,
                    BoxState::Error,
                    Some(&format!("{}: {e}", task.detail())),
                )
                .await;
        }
    }
}

/// gVisor sandbox (D89): the image roll is a container recreate on the same `/work` image —
/// seconds, nothing copied. Same status surface (`upgrading`, audit rows) as the Daytona path.
async fn start_gvisor(
    state: &AppState,
    user_id: &str,
    sb: &crate::db::Sandbox,
    force: bool,
) -> Result<bool> {
    let target = state
        .db
        .user_snapshot(user_id)?
        .unwrap_or_else(|| state.default_snapshot());
    if !force && sb.snapshot.as_deref() == Some(target.as_str()) {
        return Ok(false);
    }
    {
        let mut running = state.upgrading.lock().await;
        if running.contains_key(user_id) {
            return Ok(false);
        }
        refuse_while_restarting(state)?;
        running.insert(
            user_id.to_owned(),
            UpgradeStatus {
                phase: "recreating".into(),
                since: now_ms(),
                from_snapshot: sb.snapshot.clone(),
                to_snapshot: target.clone(),
            },
        );
    }
    let state2 = state.clone();
    let user = user_id.to_owned();
    tokio::spawn(async move {
        let t0 = Instant::now();
        state2
            .set_box_state(&user, BoxState::Waking, Some("upgrading: recreating"))
            .await;
        let outcome = crate::fleet::recreate_with_snapshot(&state2, &user, &target).await;
        state2.upgrading.lock().await.remove(&user);
        match outcome {
            Ok(()) => {
                tracing::info!(user = %user, target, ms = t0.elapsed().as_millis() as i64, "gvisor box upgraded");
                let _ = state2.db.record_audit(
                    "relay-admin",
                    "box_upgrade",
                    &json!({ "ts": now_ms(), "user_id": user, "report": { "provider": "gvisor", "to": target }, "ms": t0.elapsed().as_millis() as i64 }),
                );
            }
            Err(e) => {
                tracing::error!(user = %user, error = %format!("{e:#}"), "gvisor box upgrade failed");
                let _ = state2.db.record_audit(
                    "relay-admin",
                    "box_upgrade_failed",
                    &json!({ "ts": now_ms(), "user_id": user, "error": format!("{e:#}") }),
                );
                state2
                    .set_box_state(
                        &user,
                        BoxState::Error,
                        Some(&format!("upgrade failed: {e}")),
                    )
                    .await;
            }
        }
    });
    Ok(true)
}

/// Where the `/work` to carry comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// The user's current box (normal upgrade): it is stopped and kept as the fallback.
    CurrentBox,
    /// A retained sandbox from `box_history` (repair: the row lost its sandbox).
    History { sandbox_id: String },
}

/// Make sure the source sandbox runs (started when stopped) and hand back its toolbox. Nothing on
/// the box is stopped here.
async fn source_toolbox<'a>(
    dt: &'a crate::daytona::Daytona,
    sandbox_id: &str,
) -> Result<crate::daytona::Toolbox<'a>> {
    let cur = dt.get(sandbox_id).await.context("source sandbox")?;
    if matches!(
        cur.state,
        SandboxState::Destroyed | SandboxState::Destroying
    ) {
        return Err(anyhow!(
            "sandbox {sandbox_id} is destroyed; nothing to carry over"
        ));
    }
    if cur.state != SandboxState::Started {
        dt.start(sandbox_id).await?;
        dt.wait_for(sandbox_id, Duration::from_secs(180), |s| {
            s.state == SandboxState::Started
        })
        .await?;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    Ok(dt.toolbox(sandbox_id).await)
}

/// What `pack` carried: the spooled parts and the figures for the report and the room check.
struct Packed {
    parts: Vec<carry::SpooledPart>,
    /// `(entries, bytes)` the new box must hold afterwards.
    expected: (u64, u64),
    work_mib: i64,
    measure: carry::Measure,
    skipped_rebuildable: bool,
}

/// Quiesce agentd, list `/work`, plan the parts and carry them onto the relay's spool disk. The
/// caller has already measured and decided to go ahead.
async fn pack(
    state: &AppState,
    user_id: &str,
    tb: &crate::daytona::Toolbox<'_>,
    work_mib: i64,
    cfg: &CarryConfig,
) -> Result<Packed> {
    carry::quiesce(tb).await.context("quiesce the source box")?;
    note_phase(state, user_id, "list").await;
    let entries = carry::list_work(tb, cfg.attempts).await?;
    let measure = carry::measure(&entries);
    let parts = carry::partition(&entries, cfg.part_mib * 1024 * 1024, cfg.skip_rebuildable);
    let expected = carry::expected_after_carry(&parts);
    tracing::info!(
        user = %user_id, work_mib, entries = measure.entries, bytes = measure.bytes,
        parts = parts.len(), part_mib = cfg.part_mib,
        rebuildable_mib = measure.rebuildable_bytes / (1024 * 1024),
        rebuildable_entries = measure.rebuildable_entries,
        projects_with_copies = measure.projects_with_copies.len(),
        skip_rebuildable = cfg.skip_rebuildable,
        "source /work listed and planned"
    );
    let spool = cfg.spool_root.join(user_id);
    let n = parts.len();
    let parts = carry::pack_parts(tb, &parts, &spool, cfg, |k, _| {
        let (state, user) = (state.clone(), user_id.to_owned());
        tokio::spawn(async move { note_phase(&state, &user, &format!("pack {k}/{n}")).await });
    })
    .await?;
    Ok(Packed {
        parts,
        expected,
        work_mib,
        measure,
        skipped_rebuildable: cfg.skip_rebuildable,
    })
}

/// Put the carried `/work` onto the staged new box. The user's row still names the source.
/// `holds_data` turns true before the first byte of `/work` is uploaded: from then on the new box
/// may hold user data and rollback keeps it instead of deleting it.
async fn unpack(
    state: &AppState,
    dt: &crate::daytona::Daytona,
    user_id: &str,
    new_id: &str,
    packed: &Packed,
    cfg: &CarryConfig,
    holds_data: &mut bool,
) -> Result<()> {
    let tb = dt.toolbox(new_id).await;
    let _ = tb
        .exec(
            "sudo -n pkill -TERM -x arbos-agentd 2>/dev/null; sleep 2; sudo -n pkill -KILL -x arbos-agentd 2>/dev/null; true",
            60,
        )
        .await;
    let (avail_mib, avail_inodes) = carry::destination_room(&tb).await?;
    carry::destination_fits(
        avail_mib,
        avail_inodes,
        packed.work_mib.max(0) as u64,
        packed.expected.0,
        cfg.part_mib,
        carry::max_part_mib(&packed.parts),
    )
    .map_err(|m| anyhow!("{m}"))?;
    let n = packed.parts.len();
    *holds_data = true;
    carry::unpack_parts(&tb, &packed.parts, packed.expected, cfg, |k, _| {
        let (state, user) = (state.clone(), user_id.to_owned());
        tokio::spawn(async move { set_phase(&state, &user, &format!("unpack {k}/{n}")).await });
    })
    .await?;
    let owners = tb
        .exec(
            "sudo -n find /work -maxdepth 3 -name .owner -path '*/.arbos/.owner' -delete; echo ok",
            120,
        )
        .await?;
    if owners.get("exitCode").and_then(Value::as_i64).unwrap_or(1) != 0 {
        return Err(anyhow!(
            "owner markers on the new box: {}",
            owners.get("result").and_then(Value::as_str).unwrap_or("")
        ));
    }
    Ok(())
}

/// The swap: the carried new box becomes the user's row, then it is bounced so the entrypoint
/// starts agentd on the carried `/work`, and must say hello.
async fn swap_and_start(
    state: &AppState,
    dt: &crate::daytona::Daytona,
    user_id: &str,
    new: &crate::db::Sandbox,
    new_id: &str,
) -> Result<()> {
    set_phase(state, user_id, "start").await;
    dt.stop(new_id, true).await?;
    dt.wait_for(new_id, Duration::from_secs(120), |s| {
        s.state == SandboxState::Stopped
    })
    .await?;
    let mut row = new.clone();
    row.daytona_state = Some("stopped".into());
    state.db.upsert_box(&row)?;
    state.set_box_state(user_id, BoxState::Asleep, None).await;
    boxes::start_box(state, user_id).await?;
    // The new box must say hello; otherwise the upgrade did not work.
    let deadline = Instant::now() + Duration::from_secs(150);
    loop {
        let (st, _) = state.box_state(user_id).await;
        if st == BoxState::Awake {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(anyhow!("the new box did not connect within 150 s"));
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// The new sandbox of a failed run, as far as it got.
pub(crate) struct Staged<'a> {
    pub(crate) row: &'a crate::db::Sandbox,
    /// Part of `/work` was uploaded to it, or the row named it: it is kept (stopped, recorded for
    /// the sweeper), never deleted on the spot.
    pub(crate) keep: bool,
    /// The swap wrote it into the user's row.
    pub(crate) swapped: bool,
}

/// Write the row the user had before the run, verify it landed, retry a few times. Returns
/// whether the row is back.
async fn put_row_back(state: &AppState, user_id: &str, prev: Option<&crate::db::Sandbox>) -> bool {
    let want = prev.and_then(|p| p.sandbox_id.clone());
    for attempt in 0..5u32 {
        let wrote = match prev {
            Some(p) if p.sandbox_id.is_some() => state.db.upsert_box(p),
            Some(p) => state.db.upsert_box_clearing_sandbox(p),
            None => state.db.delete_box_row(user_id).map(|_| ()),
        };
        let now = state
            .db
            .user_box(user_id)
            .map(|r| r.and_then(|r| r.sandbox_id));
        match (wrote, now) {
            (Ok(()), Ok(now)) if now == want => return true,
            (w, n) => tracing::warn!(
                user = %user_id, attempt, write = ?w.err(), row_sandbox = ?n.ok().flatten(), want = ?want,
                "rollback: the box row is not back on the source yet"
            ),
        }
        tokio::time::sleep(Duration::from_millis(200 << attempt)).await;
    }
    false
}

/// Undo a failed run. Order matters: the row goes back first, cleanup comes after, so no failure
/// in the cleanup can leave the source without a row.
///   1. Upgrade: the row is put back on the source (verified, retried) and the source leaves
///      `box_history`. If the row cannot be written, the source stays in `box_history` (findable,
///      re-adoptable by repair). Repair: a row the swap wrote is put back as it was.
///   2. The new sandbox: deleted only when it never was the row's sandbox and nothing of `/work`
///      was uploaded to it; otherwise stopped and recorded as a partial history entry that the
///      sweeper deletes after the fallback window. A failed delete is recorded the same way, due now.
///   3. Upgrade: the source is started again.
pub(crate) async fn rollback(
    state: &AppState,
    dt: &crate::daytona::Daytona,
    user_id: &str,
    source: &Source,
    prev_row: Option<&crate::db::Sandbox>,
    staged: Option<Staged<'_>>,
) {
    let new_sandbox = staged.as_ref().and_then(|s| s.row.sandbox_id.clone());
    tracing::warn!(user = %user_id, new_sandbox, "upgrade rolled back");
    match source {
        Source::CurrentBox => {
            let restored = prev_row.map(|old| {
                let mut r = old.clone();
                r.daytona_state = Some("stopped".into());
                r
            });
            if restored.is_none() {
                tracing::error!(user = %user_id, "rollback has no source row to put back");
            } else if put_row_back(state, user_id, restored.as_ref()).await {
                if let Some(old_id) = prev_row.and_then(|o| o.sandbox_id.as_deref()) {
                    let _ = state.db.remove_box_history(old_id);
                }
            } else {
                tracing::error!(user = %user_id, source = ?prev_row.and_then(|o| o.sandbox_id.clone()), "rollback could not put the box row back on the source; the source stays in box_history for repair");
            }
        }
        Source::History { .. } => {
            if staged.as_ref().is_some_and(|s| s.swapped)
                && !put_row_back(state, user_id, prev_row).await
            {
                tracing::error!(user = %user_id, "repair rollback could not put the box row back");
            }
        }
    }

    if let Some(s) = &staged {
        if s.keep || s.swapped {
            if let Some(id) = s.row.sandbox_id.as_deref() {
                if let Err(e) = dt.stop(id, true).await {
                    tracing::warn!(user = %user_id, sandbox = %id, error = %e, "rollback: stop of the new box failed; recorded for the sweeper");
                }
            }
            if let Err(e) = state
                .db
                .insert_partial_box_history(s.row, now_ms() + KEEP_OLD_SANDBOX_MS)
            {
                tracing::error!(user = %user_id, sandbox = ?s.row.sandbox_id, error = %e, "rollback could not record the new box in box_history");
            }
        } else {
            boxes::discard_empty_staged(state, dt, s.row).await;
        }
    }

    if matches!(source, Source::CurrentBox) {
        let _ = state.db.set_box_held(user_id, false);
        state
            .set_box_state(user_id, BoxState::Asleep, Some("upgrade rolled back"))
            .await;
        if let Err(e) = boxes::start_box(state, user_id).await {
            tracing::error!(user = %user_id, error = %e, "restart of the old box after rollback failed");
        }
    }
}

/// The pack step failed after agentd was quiesced but before the old sandbox was stopped or
/// anything created: bring the user's box back as it was (start_box bounces a started-but-silent
/// sandbox so the entrypoint starts agentd again).
async fn revive_source(state: &AppState, user_id: &str, source: &Source) {
    if !matches!(source, Source::CurrentBox) {
        return;
    }
    tracing::warn!(user = %user_id, "pack failed; reviving the box in place");
    state
        .set_box_state(user_id, BoxState::Asleep, Some("upgrade did not start"))
        .await;
    if let Err(e) = boxes::start_box(state, user_id).await {
        tracing::error!(user = %user_id, error = %e, "revive of the box after a failed pack failed");
    }
}

async fn run(state: &AppState, user_id: &str, target: &str, source: Source) -> Result<Outcome> {
    let dt = state
        .daytona
        .as_ref()
        .ok_or_else(|| anyhow!("DAYTONA_API_KEY not configured"))?;
    let cfg = CarryConfig::from_env(&state.cfg.data_dir);
    let (source_id, old_row) = match &source {
        Source::CurrentBox => {
            let old = state
                .db
                .user_box(user_id)?
                .ok_or_else(|| anyhow!("box vanished"))?;
            let id = old
                .sandbox_id
                .clone()
                .ok_or_else(|| anyhow!("the box row has no sandbox id; use the repair path"))?;
            (id, Some(old))
        }
        Source::History { sandbox_id } => (sandbox_id.clone(), None),
    };

    // Measure first: nothing on the box is stopped until the carry is known to fit.
    note_phase(state, user_id, "measure").await;
    let tb = source_toolbox(dt, &source_id).await?;
    let work_mib = carry::measure_du_mib(&tb).await?;
    let spool_free = carry::free_mib(&cfg.spool_root).unwrap_or(0);
    match carry::verdict(work_mib, spool_free, &cfg) {
        Verdict::Carry => {}
        v => {
            let reason = v.reason().unwrap_or_default();
            return Ok(Outcome::Refused { reason, work_mib });
        }
    }

    // One carry at a time on the toolbox link (#2254): a second carry halves the throughput for
    // both and every piece times out. Waiting here keeps the box running; nothing is stopped yet.
    let permits = carry::carry_permits(&cfg);
    if permits.available_permits() == 0 {
        set_phase(state, user_id, "wait").await;
        tracing::info!(user = %user_id, "another /work carry is running; waiting for the toolbox link");
    }
    let _carry_permit = permits
        .acquire()
        .await
        .map_err(|_| anyhow!("carry permits closed"))?;

    set_phase(state, user_id, "pack").await;
    let packed = match pack(state, user_id, &tb, work_mib, &cfg).await {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_dir_all(cfg.spool_root.join(user_id));
            revive_source(state, user_id, &source).await;
            return Err(e.context("pack /work on the source box"));
        }
    };
    tracing::info!(
        user = %user_id, mib = packed.work_mib, parts = packed.parts.len(),
        tar_bytes = packed.parts.iter().map(|p| p.tar_bytes).sum::<u64>(),
        "source /work packed"
    );
    let spool_dir = cfg.spool_root.join(user_id);
    let outcome = carry_onto_new_box(
        state, dt, user_id, target, &source, &source_id, old_row, &packed, &cfg,
    )
    .await;
    let _ = std::fs::remove_dir_all(&spool_dir);
    outcome
}

/// Stop the old sandbox, create the new one staged (the row stays on the source), unpack onto it,
/// then swap the row over and start it. Every failure rolls back: the row ends where it began.
#[allow(clippy::too_many_arguments)]
async fn carry_onto_new_box(
    state: &AppState,
    dt: &crate::daytona::Daytona,
    user_id: &str,
    target: &str,
    source: &Source,
    source_id: &str,
    old_row: Option<crate::db::Sandbox>,
    packed: &Packed,
    cfg: &CarryConfig,
) -> Result<Outcome> {
    let prev_row = match &old_row {
        Some(old) => Some(old.clone()),
        None => state.db.user_box(user_id)?,
    };
    if let Some(old) = &old_row {
        set_phase(state, user_id, "stop").await;
        let _ = dt.stop(source_id, false).await;
        if let Err(e) = state
            .db
            .insert_box_history(old, now_ms() + KEEP_OLD_SANDBOX_MS)
        {
            rollback(state, dt, user_id, source, prev_row.as_ref(), None).await;
            return Err(e.context("record the source box in box_history"));
        }
    }

    set_phase(state, user_id, "create").await;
    let mut new = match boxes::create_staged_box(state, user_id, Some(target)).await {
        Ok(n) => n,
        Err(e) => {
            rollback(state, dt, user_id, source, prev_row.as_ref(), None).await;
            return Err(e.context("create the new box"));
        }
    };
    if let Some(old) = &old_row {
        new.migrated_from = old.migrated_from.clone();
    }
    let Some(new_id) = new.sandbox_id.clone() else {
        rollback(state, dt, user_id, source, prev_row.as_ref(), None).await;
        return Err(anyhow!("new box has no sandbox id"));
    };

    set_phase(state, user_id, "unpack").await;
    let mut holds_data = false;
    if let Err(e) = unpack(state, dt, user_id, &new_id, packed, cfg, &mut holds_data).await {
        let staged = Staged {
            row: &new,
            keep: holds_data,
            swapped: false,
        };
        rollback(state, dt, user_id, source, prev_row.as_ref(), Some(staged)).await;
        return Err(e.context("carry /work onto the new box"));
    }

    if let Err(e) = swap_and_start(state, dt, user_id, &new, &new_id).await {
        let staged = Staged {
            row: &new,
            keep: true,
            swapped: true,
        };
        rollback(state, dt, user_id, source, prev_row.as_ref(), Some(staged)).await;
        return Err(e.context("start the new box"));
    }
    let archived = archive_excess_retained(state, dt, Some(user_id)).await;
    Ok(Outcome::Done(json!({
        "source_sandbox": source_id, "new_sandbox": new_id, "archived_retained": archived,
        "from": old_row.as_ref().and_then(|o| o.snapshot.clone()), "to": target,
        "work_mib": packed.work_mib, "parts": packed.parts.len(), "part_mib": cfg.part_mib,
        "tar_bytes": packed.parts.iter().map(|p| p.tar_bytes).sum::<u64>(),
        "entries": packed.expected.0, "bytes": packed.expected.1,
        "rebuildable": {
            "entries": packed.measure.rebuildable_entries, "bytes": packed.measure.rebuildable_bytes,
            "projects_with_copies": packed.measure.projects_with_copies,
            "skipped": packed.skipped_rebuildable,
        },
        "old_kept_until": old_row.as_ref().map(|_| now_ms() + KEEP_OLD_SANDBOX_MS),
        "repair": matches!(source, Source::History { .. }),
    })))
}

/// Daytona-archive every retained sandbox beyond the [`KEEP_UNARCHIVED_RETAINED`] newest (one user, or
/// all when `user_id` is `None`). Archive, never delete: `start` restores an archived sandbox (slower —
/// see the runbook); the fallback window and the delete sweeper are unchanged. Returns how many were
/// archived; a sandbox that is still running is stopped and left for the hourly sweeper.
pub async fn archive_excess_retained(
    state: &AppState,
    dt: &crate::daytona::Daytona,
    user_id: Option<&str>,
) -> usize {
    let due = state
        .db
        .box_history_to_archive(user_id, KEEP_UNARCHIVED_RETAINED)
        .unwrap_or_default();
    let mut n = 0;
    for (uid, sandbox_id) in due {
        match dt.archive(&sandbox_id).await {
            Ok(_) => {
                let _ = state.db.mark_box_history_archived(&sandbox_id);
                tracing::info!(user = %uid, sandbox = %sandbox_id, "retained box archived (beyond the newest {KEEP_UNARCHIVED_RETAINED})");
                n += 1;
            }
            Err(e) if e.to_string().contains("404") => {
                let _ = state.db.mark_box_history_deleted(&sandbox_id);
            }
            // Daytona archives stopped sandboxes only (400 "Sandbox is not stopped"); a retained box
            // the upgrade left running is stopped here and archived on the next pass.
            Err(e) if e.to_string().contains("not stopped") => {
                match dt.stop(&sandbox_id, false).await {
                    Ok(_) => {
                        tracing::info!(user = %uid, sandbox = %sandbox_id, "retained box stopped; archived on the next pass")
                    }
                    Err(e) => {
                        tracing::warn!(user = %uid, sandbox = %sandbox_id, error = %e, "retained box stop before archive")
                    }
                }
            }
            Err(e) => {
                tracing::warn!(user = %uid, sandbox = %sandbox_id, error = %e, "retained box archive")
            }
        }
    }
    n
}

/// Repair: the user's row has no usable sandbox (a failed upgrade, a deleted sandbox) but a
/// retained one exists in `box_history`. Carry its `/work` onto a fresh box from the default
/// snapshot. `Ok(false)` when nothing is wrong or a run is in progress.
pub async fn start_repair(state: &AppState, user_id: &str) -> Result<bool> {
    let row = state.db.user_box(user_id)?;
    let dt = state
        .daytona
        .as_ref()
        .ok_or_else(|| anyhow!("DAYTONA_API_KEY not configured"))?;
    let usable = match row.as_ref().and_then(|r| r.sandbox_id.clone()) {
        Some(id) => !matches!(
            dt.get(&id).await.map(|s| s.state),
            Err(_) | Ok(SandboxState::Destroyed) | Ok(SandboxState::Destroying)
        ),
        None => false,
    };
    if usable {
        return Ok(false);
    }
    let retained = state
        .db
        .latest_box_history(user_id)?
        .ok_or_else(|| anyhow!("no retained sandbox for {user_id}"))?;
    // Cheapest repair: the history entry kept the full row (token hash, device id, urls) and the
    // sandbox still exists — put the row back and start it; no copy needed.
    if let Some(mut old_row) = retained.row.clone() {
        if dt.get(&retained.sandbox_id).await.is_ok() {
            old_row.daytona_state = Some("stopped".into());
            state.db.upsert_box(&old_row)?;
            state.db.set_box_held(user_id, false)?;
            state.db.remove_box_history(&retained.sandbox_id)?;
            state
                .set_box_state(
                    user_id,
                    BoxState::Asleep,
                    Some("re-adopted the retained box"),
                )
                .await;
            boxes::ensure_awake(state, user_id, "start").await;
            let _ = state.db.record_audit(
                "relay-admin",
                "box_readopt",
                &json!({ "ts": now_ms(), "user_id": user_id, "sandbox": retained.sandbox_id }),
            );
            tracing::info!(user = %user_id, sandbox = %retained.sandbox_id, "retained box re-adopted");
            return Ok(true);
        }
    }
    let (sandbox_id, snapshot) = (retained.sandbox_id, retained.snapshot);
    let target = state
        .db
        .user_snapshot(user_id)?
        .unwrap_or_else(|| state.default_snapshot());
    {
        let mut running = state.upgrading.lock().await;
        if running.contains_key(user_id) {
            return Ok(false);
        }
        refuse_while_restarting(state)?;
        running.insert(
            user_id.to_owned(),
            UpgradeStatus {
                phase: "repair".into(),
                since: now_ms(),
                from_snapshot: snapshot,
                to_snapshot: target.clone(),
            },
        );
    }
    let state2 = state.clone();
    let user = user_id.to_owned();
    tokio::spawn(async move {
        let t0 = Instant::now();
        let outcome = run(&state2, &user, &target, Source::History { sandbox_id }).await;
        state2.upgrading.lock().await.remove(&user);
        finish(&state2, &user, outcome, Task::Repair, t0).await;
    });
    Ok(true)
}

/// `POST /admin/users/{id}/box/repair`.
pub async fn admin_repair(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    match start_repair(&state, &id).await {
        Ok(true) => (
            StatusCode::ACCEPTED,
            Json(json!({ "user_id": id, "upgrade": status_json(&state, &id).await })),
        )
            .into_response(),
        Ok(false) => Json(json!({ "user_id": id, "upgrade": status_json(&state, &id).await, "note": "nothing to repair (the box row has a live sandbox) or a run is in progress" })).into_response(),
        Err(e) if e.to_string().contains(RESTARTING_PHRASE) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "restarting", e.to_string())
        }
        Err(e) => api_error(StatusCode::BAD_GATEWAY, "repair", e.to_string()),
    }
}

/// Hourly: delete old sandboxes whose fallback window has passed.
pub async fn spawn_history_sweeper(state: AppState) {
    let Some(_) = state.daytona.as_ref() else {
        return;
    };
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let Some(dt) = state.daytona.as_ref() else {
                return;
            };
            // Anything the per-upgrade pass could not archive (stopped only on that pass, API hiccup).
            archive_excess_retained(&state, dt, None).await;
            for (user_id, sandbox_id) in state.db.box_history_due().unwrap_or_default() {
                match dt.delete(&sandbox_id).await {
                    Ok(_) => {
                        tracing::info!(user = %user_id, sandbox = %sandbox_id, "old box deleted after the fallback window");
                        let _ = state.db.mark_box_history_deleted(&sandbox_id);
                    }
                    Err(e) if e.to_string().contains("404") => {
                        let _ = state.db.mark_box_history_deleted(&sandbox_id);
                    }
                    Err(e) => tracing::warn!(sandbox = %sandbox_id, error = %e, "old box delete"),
                }
            }
        }
    });
}

// ---- handlers -----------------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct UpgradeQuery {
    /// Recreate even when the snapshot already matches.
    #[serde(default)]
    pub force: bool,
}

async fn kick(state: &AppState, user_id: &str, force: bool) -> Response {
    match start(state, user_id, force).await {
        Ok(true) => (
            StatusCode::ACCEPTED,
            Json(json!({ "user_id": user_id, "upgrade": status_json(state, user_id).await })),
        )
            .into_response(),
        Ok(false) => {
            let st = status_json(state, user_id).await;
            if st["in_progress"].as_bool() == Some(true) {
                (
                    StatusCode::ACCEPTED,
                    Json(json!({ "user_id": user_id, "upgrade": st })),
                )
                    .into_response()
            } else {
                Json(json!({ "user_id": user_id, "upgrade": st, "note": "already on the current snapshot (use ?force=true to recreate anyway)" }))
                    .into_response()
            }
        }
        Err(e) if e.to_string().contains(RESTARTING_PHRASE) => {
            api_error(StatusCode::SERVICE_UNAVAILABLE, "restarting", e.to_string())
        }
        Err(e) => api_error(StatusCode::BAD_GATEWAY, "upgrade", e.to_string()),
    }
}

/// `POST /machine/upgrade[?force=true]` — the caller's own box.
pub async fn machine_upgrade(
    State(state): State<AppState>,
    principal: Principal,
    axum::extract::Query(q): axum::extract::Query<UpgradeQuery>,
) -> Response {
    let Principal::Device { user_id, .. } = &principal else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "a signed-in device is required",
        );
    };
    if let Err(r) = crate::credits::check(&state, user_id) {
        return crate::credits::refused_response(&r);
    }
    kick(&state, user_id, q.force).await
}

/// `POST /admin/users/{id}/box/upgrade[?force=true]`.
pub async fn admin_upgrade(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<UpgradeQuery>,
) -> Response {
    if !matches!(principal, Principal::Admin) {
        return api_error(StatusCode::FORBIDDEN, "forbidden", "admin token required");
    }
    kick(&state, &id, q.force).await
}
