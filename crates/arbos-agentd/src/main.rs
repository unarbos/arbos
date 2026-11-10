#[cfg(test)]
mod ack_is_not_an_answer_tests;
mod activity;
mod agent;
mod agent_messages;
mod approval;
mod attachments;
mod budget;
#[cfg(test)]
mod cancel_handles_tests;
mod channels;
mod chart;
mod config;
mod connectors;
#[cfg(test)]
mod crash_loop_leftover_tests;
#[cfg(test)]
mod cross_workspace_auth_tests;
mod disk;
mod eval;
mod feedback;
mod forced_job;
#[cfg(test)]
mod forced_job_burst_tests;
#[cfg(test)]
mod forced_job_tests;
mod goals;
mod groups;
mod handoff;
mod harness;
mod hooks;
mod http;
mod identity;
mod inspect;
mod isolation;
#[cfg(test)]
mod job_background_claim_tests;
mod job_backgrounded;
#[cfg(test)]
mod job_cancel_real_worker_tests;
#[cfg(test)]
mod job_cancel_tests;
#[cfg(test)]
mod job_cancel_unconfirmed_tests;
#[cfg(test)]
mod job_cancel_window_tests;
#[cfg(test)]
mod job_correction_routing_tests;
#[cfg(test)]
mod job_finished_sweep_tests;
#[cfg(test)]
mod job_handoff_leak_tests;
#[cfg(test)]
mod job_leftover_tests;
#[cfg(test)]
mod job_list_cap_tests;
#[cfg(test)]
mod job_note_tests;
#[cfg(test)]
mod job_notice_tests;
#[cfg(test)]
mod job_one_file_per_ask_tests;
#[cfg(test)]
mod job_orphan_tests;
#[cfg(test)]
mod job_queued_cancel_tests;
#[cfg(test)]
mod job_reparent_tests;
#[cfg(test)]
mod job_requeue_order_tests;
#[cfg(test)]
mod job_result_budget_tests;
#[cfg(test)]
mod job_steer_inherit_tests;
#[cfg(test)]
mod job_steer_race_tests;
#[cfg(test)]
mod job_wait_tier_tests;
mod jobs_view;
mod llm;
mod local;
mod markdown_tables;
mod mcp;
mod media;
mod memory;
#[cfg(test)]
mod model_switch_tests;
mod people;
#[cfg(test)]
mod pi_crash_loop_tests;
#[cfg(test)]
mod refusal_line_and_window_notice_tests;
#[cfg(test)]
mod refused_turn_window_tests;
mod relay_error;
mod reply;
mod resources;
mod responders;
#[cfg(test)]
mod room_marks_tests;
mod routines;
#[cfg(test)]
mod rowless_cancel_entrypoint_tests;
#[cfg(test)]
mod rowless_cancel_evidence_tests;
#[cfg(test)]
mod rowless_cancel_tests;
mod rpc_child;
mod selfref;
mod send_order;
#[cfg(test)]
mod send_order_tests;
mod settings;
mod silence;
mod sites;
mod skills;
mod status;
mod steer_gate;
mod store;
#[cfg(test)]
mod sweep_contention_tests;
mod sync;
mod tree;
mod truncate;
#[cfg(test)]
mod turn_end_race_tests;
#[cfg(test)]
mod turn_marks_tests;
mod vault;
mod voice;
mod web;
#[cfg(test)]
mod worker_model_refusal_tests;
#[cfg(test)]
mod worker_retention_tests;
mod workspace;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use arbos_core::relay_url::SignedRelayUrl;
use base64::Engine;
use clap::{Parser, Subcommand};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde_json::{json, Value};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::BoxConfig;
use crate::workspace::Supervisor;

#[derive(Parser, Debug)]
#[command(name = "arbos-agentd", version, about = "Arbos box entrypoint")]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Boot the box: doc host, main agent, MCP, history store (image ENTRYPOINT).
    Boot,
    /// Print the effective configuration (secrets masked).
    Config,
    /// Replay a labelled group-chat fixture through the respond judge and report exact-set
    /// accuracy, per-agent precision/recall, silence and exactly-one accuracy, a threshold
    /// sweep, latency, tokens and cost. Needs OPENROUTER_API_KEY, or ARBOS_RELAY_URL +
    /// ARBOS_BOX_TOKEN for the relay proxy.
    EvalResponders {
        /// Fixture path (default: the real-Arbonites set).
        #[arg(
            long,
            default_value = "crates/arbos-agentd/tests/fixtures/respond-gate-arbonites.json"
        )]
        fixture: String,
        /// Decision-model id (default typesafe/jev-1.14).
        #[arg(long)]
        scorer: Option<String>,
        /// Confidence threshold (default the gate's own).
        #[arg(long)]
        threshold: Option<f64>,
        /// Run every case this many times (latency spread, determinism check).
        #[arg(long, default_value_t = 1)]
        repeat: usize,
        /// Write the JSON report here instead of stdout.
        #[arg(long)]
        out: Option<String>,
    },
    /// Memory tree (D77) offline check: build the tree over a copy of a workspace's `agentd.db`
    /// (cards through OpenRouter or the relay proxy, same env rules as eval-responders), walk one
    /// query as if asked in a new chat, and print the pack with every decision.
    EvalTree {
        /// Path to a copy of `<workspace>/.arbos/agentd/agentd.db` (the copy gets the tree tables).
        #[arg(long)]
        db: String,
        /// The question, as the user would type it in a new chat.
        #[arg(long)]
        query: String,
        /// The chat the question is asked in (its own recent window is excluded).
        #[arg(long, default_value = "new-chat")]
        chat: String,
        #[arg(long, default_value = "typesafe/jev-1.14")]
        scorer: String,
        #[arg(long, default_value = "anthropic/claude-haiku-5.5")]
        card_model: String,
        #[arg(long)]
        out: Option<String>,
        /// Pack with the compact policy (`ARBOS_MEMORY_PACK_COMPACT`) instead of the full one.
        #[arg(long)]
        compact: bool,
        /// Walk the D77 way (every thread scored, three Jev rounds, no cache) instead of the
        /// fast walk (`ARBOS_MEMORY_WALK_FAST`).
        #[arg(long)]
        slow: bool,
    },
    /// Mark historical provider-refused user turns that predate live marking (#2925) and rebuild
    /// the memory cards that summarised them, through the live mark/invalidate path. Mark-only:
    /// never deletes or rewrites message content. Idempotent. No model calls.
    BackfillRefused {
        /// Path to the workspace's `agentd.db`.
        #[arg(long)]
        db: String,
        /// Count what a live run would mark and change nothing. Report this first.
        #[arg(long)]
        dry_run: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,loro_internal=warn,arbos_agentd=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .with(inspect::WarnRing)
        .init();
    let args = Args::parse();
    match args.cmd {
        Cmd::Boot => {
            // Daytona starts us as `daytona`; per-workspace users need root (sudo is passwordless).
            let local_box = std::env::var("ARBOS_LOCAL_BOX").is_ok_and(|v| v == "1");
            isolation::reexec_as_root_if_needed(local_box);
            boot().await
        }
        Cmd::Config => {
            let cfg = BoxConfig::from_env()?;
            println!(
                "{}",
                json!({ "user_id": cfg.user_id, "relay_url": cfg.relay_url, "work_root": cfg.work_root, "port": cfg.port, "local_box": cfg.local_box, "box_token": mask(&cfg.box_token), "mcp_token": mask(&cfg.mcp_token), "default_talking_model": cfg.default_talking_model, "default_subagent_model": cfg.default_subagent_model, "legacy_project_id": cfg.legacy_project_id })
            );
            Ok(())
        }
        Cmd::EvalResponders {
            fixture,
            scorer,
            threshold,
            repeat,
            out,
        } => {
            eval::run(eval::EvalArgs {
                fixture,
                scorer,
                threshold,
                repeat,
                out,
            })
            .await
        }
        Cmd::EvalTree {
            db,
            query,
            chat,
            scorer,
            card_model,
            out,
            compact,
            slow,
        } => {
            let opts = tree::WalkOptions {
                compact,
                fast: !slow,
            };
            eval_tree(db, query, chat, scorer, card_model, out, opts).await
        }
        Cmd::BackfillRefused { db, dry_run } => backfill_refused(db, dry_run),
    }
}

/// Mark historical provider-refused turns and rebuild their cards (`Tree::backfill_refused`).
/// Opens the store directly; no relay or model is needed, so the `Tree` carries a placeholder
/// `Llm` the backfill never calls.
fn backfill_refused(db: String, dry_run: bool) -> Result<()> {
    let store = store::Store::open(std::path::Path::new(&db))?;
    let llm = llm::Llm::with_endpoints("", "", "");
    let tree = tree::Tree::with_ids(
        "backfill",
        "backfill",
        settings::LiveModels::fixed("none"),
        store,
        std::sync::Arc::new(llm),
    );
    let r = tree.backfill_refused(dry_run)?;
    println!(
        "{}",
        json!({
            "dry_run": r.dry_run,
            "found": r.found,
            "marked": r.marked,
            "rebuilt": r.rebuilt,
        })
    );
    Ok(())
}

async fn eval_tree(
    db: String,
    query: String,
    chat: String,
    scorer: String,
    card_model: String,
    out: Option<String>,
    opts: tree::WalkOptions,
) -> Result<()> {
    let llm = match std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(key) => llm::Llm::with_endpoints(
            "https://openrouter.ai/api/v1",
            "https://openrouter.ai/api/alpha/decisions",
            &key,
        ),
        None => {
            let relay = std::env::var("ARBOS_RELAY_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("set OPENROUTER_API_KEY, or ARBOS_RELAY_URL + ARBOS_BOX_TOKEN")
                })?;
            let token = std::env::var("ARBOS_BOX_TOKEN").unwrap_or_default();
            let relay = relay.trim_end_matches('/');
            llm::Llm::with_endpoints(
                &format!("{relay}/proxy/v1"),
                &format!("{relay}/proxy/decisions"),
                &token,
            )
        }
    };
    let store = store::Store::open(std::path::Path::new(&db))?;
    let tree = tree::Tree::with_ids(
        "eval",
        "eval",
        settings::LiveModels::fixed(&card_model),
        store,
        std::sync::Arc::new(llm),
    );
    let report = tree.eval(&scorer, &chat, &query, opts).await?;
    let text = serde_json::to_string_pretty(&report)?;
    match out {
        Some(p) => std::fs::write(p, text)?,
        None => println!("{text}"),
    }
    Ok(())
}

fn mask(s: &str) -> String {
    if s.len() > 8 {
        format!("{}…({} chars)", &s[..6], s.len())
    } else {
        "(unset)".into()
    }
}

/// Where this box may be told to find the relay, and how much that telling is worth.
struct RelayUrlSource {
    /// Paths holding an address the relay pushed, newest-looking first.
    files: Vec<PathBuf>,
    /// The address baked into the create-time environment. The relay wrote it into the
    /// environment of a root process, so nothing inside the box can rewrite it — it is a
    /// trustworthy candidate, not a last resort.
    env_url: Option<String>,
    /// The public key this box verifies pushed addresses against, pinned in that same
    /// environment. `None` on a box created by a relay that cannot sign.
    verify_key: Option<VerifyingKey>,
    /// Who the address must have been signed for.
    box_id: String,
}

/// One address worth probing, and where its authority comes from.
struct Candidate {
    url: String,
    /// `Some` for a verified signature, carrying its counter; `None` for the create-time
    /// environment, which needs no signature because the attacker cannot write it either.
    signed_counter: Option<i64>,
}

fn verify_key_from_env() -> Option<VerifyingKey> {
    let raw = std::env::var(arbos_core::relay_url::VERIFY_KEY_ENV).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .ok()?;
    let bytes: [u8; 32] = bytes.try_into().ok()?;
    match VerifyingKey::from_bytes(&bytes) {
        Ok(k) => Some(k),
        Err(e) => {
            tracing::error!(error = %e, "{} is not an Ed25519 public key", arbos_core::relay_url::VERIFY_KEY_ENV);
            None
        }
    }
}

impl RelayUrlSource {
    fn from_env() -> Self {
        // The file lives in the sandbox user's home because the relay's push could not write
        // under the root-owned `/work` (D70 isolation); the old location is still read for boxes
        // created before the move.
        let file = PathBuf::from(
            std::env::var("ARBOS_RELAY_URL_FILE")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "/home/daytona/.arbos/relay-url".to_owned()),
        );
        let legacy_file = PathBuf::from(format!(
            "{}/.arbos/relay-url",
            std::env::var("ARBOS_WORK_DIR").unwrap_or_else(|_| "/work".into())
        ));
        Self {
            files: vec![file, legacy_file],
            env_url: std::env::var("ARBOS_RELAY_URL")
                .ok()
                .map(|u| u.trim().trim_end_matches('/').to_owned())
                .filter(|u| !u.is_empty()),
            verify_key: verify_key_from_env(),
            box_id: std::env::var("ARBOS_USER_ID").unwrap_or_default(),
        }
    }

    /// Read one pushed address and decide whether this box may act on it.
    ///
    /// With a key pinned, the authority is the signature beside the file and not the file: the
    /// signed payload carries the address, an unsigned or unverifiable file is refused outright,
    /// and `/healthz` is left doing the only job it can do, which is liveness. Without a key —
    /// a box created by a relay that cannot sign — the plain file is read as it always was, so
    /// that this change does not strand boxes already in the field.
    fn read_file(&self, path: &Path, now_ms: i64) -> Option<Candidate> {
        let Some(key) = self.verify_key.as_ref() else {
            let url = std::fs::read_to_string(path)
                .ok()
                .map(|u| u.trim().trim_end_matches('/').to_owned())
                .filter(|u| !u.is_empty())?;
            return Some(Candidate {
                url,
                signed_counter: None,
            });
        };
        let sig_path = PathBuf::from(format!(
            "{}{}",
            path.display(),
            arbos_core::relay_url::SIG_FILE_SUFFIX
        ));
        let raw = std::fs::read_to_string(&sig_path).ok()?;
        let signed: SignedRelayUrl = match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %sig_path.display(), error = %e, "relay url signature is not readable; refusing it");
                return None;
            }
        };
        if let Err(why) = signed.check(&self.box_id, now_ms) {
            tracing::warn!(
                path = %sig_path.display(),
                url = %signed.url,
                reason = why.as_str(),
                "refusing a pushed relay address"
            );
            return None;
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(signed.sig.trim())
            .ok()
            .and_then(|b| Signature::from_slice(&b).ok());
        let Some(sig) = decoded else {
            tracing::warn!(path = %sig_path.display(), "relay url signature is malformed; refusing it");
            return None;
        };
        if key.verify(signed.message().as_bytes(), &sig).is_err() {
            // Reaching an address proves only that something is listening. This is the check that
            // says it is ours, and it runs offline, before anything is sent.
            tracing::error!(
                path = %sig_path.display(),
                url = %signed.url,
                "a relay address on this box is not signed by our relay; refusing it"
            );
            return None;
        }
        Some(Candidate {
            url: signed.trimmed_url().to_owned(),
            signed_counter: Some(signed.counter),
        })
    }

    /// Everything worth probing, best first: the newest verified address, then the create-time
    /// environment. Ordering by counter is what stops a single stale write from being preferred
    /// to a fresher one for the life of the box.
    fn candidates(&self, now_ms: i64) -> Vec<Candidate> {
        let mut out: Vec<Candidate> = self
            .files
            .iter()
            .filter_map(|p| self.read_file(p, now_ms))
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.signed_counter));
        if let Some(env) = &self.env_url {
            out.push(Candidate {
                url: env.clone(),
                signed_counter: None,
            });
        }
        let mut seen = Vec::new();
        out.retain(|c| {
            let fresh = !seen.contains(&c.url);
            if fresh {
                seen.push(c.url.clone());
            }
            fresh
        });
        out
    }
}

/// The relay URL baked into the sandbox env goes stale when the relay's public URL changes (quick
/// tunnels change on every restart), so the relay pushes the current one into the box at every
/// wake. Returns the first address that is both ours and answers `/healthz`.
///
/// agentd does not write any of these files. It cannot sign one, and writing back the address it
/// adopted is what used to make a single bad write outlive every restart.
async fn resolve_relay_url() -> String {
    let src = RelayUrlSource::from_env();
    if src.verify_key.is_none() {
        tracing::warn!(
            "no {} in this box's environment: a pushed relay address cannot be told from any \
             other host that answers /healthz",
            arbos_core::relay_url::VERIFY_KEY_ENV
        );
    }
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client");
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let candidates = src.candidates(chrono::Utc::now().timestamp_millis());
        for c in &candidates {
            let ok = http
                .get(format!("{}/healthz", c.url))
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if ok {
                if src.env_url.as_deref() != Some(c.url.as_str()) {
                    // Distinct from the ordinary rotation note below, and from nothing at all: a
                    // box that has been pointed somewhere other than where it was created leaves
                    // a record naming both addresses.
                    tracing::warn!(
                        event = "relay_url_differs_from_create_time",
                        adopted = %c.url,
                        created_with = ?src.env_url,
                        signed_counter = ?c.signed_counter,
                        "this box is using a relay address it was not created with"
                    );
                } else {
                    tracing::info!(url = %c.url, "relay address is the one this box was created with");
                }
                return c.url.clone();
            }
        }
        if attempt == 1 || attempt % 15 == 0 {
            let urls: Vec<&str> = candidates.iter().map(|c| c.url.as_str()).collect();
            tracing::warn!(
                ?urls,
                attempt,
                "no relay url answers /healthz; waiting for the relay to push one"
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn boot() -> Result<()> {
    let t_boot = Instant::now();
    let relay_url = resolve_relay_url().await;
    // Config reads the env; the resolved URL wins over whatever the sandbox was created with.
    std::env::set_var("ARBOS_RELAY_URL", &relay_url);
    let cfg = BoxConfig::from_env()?;
    std::fs::create_dir_all(cfg.box_state_dir().join("logs"))?;
    // Disk headroom (#1641): arm the ballast and keep it armed while there is room.
    disk::boot(&cfg.work_root)?;
    disk::spawn_ballast_keeper(cfg.work_root.clone());
    // Settings → Machine: CPU / memory / disk / network of the box and of every workspace,
    // sampled every 5 s for `GET /box/state`.
    resources::start(cfg.work_root.clone());
    let sup = Supervisor::new(cfg.clone());

    // Box HTTP (health, per-workspace history/metrics, browser stream) first: the relay's preview
    // URL and the stale-box detector look here. It runs on a runtime of its own, so a boot that
    // holds this runtime for minutes cannot keep the health surface from answering (2026-11-09).
    let main_l = std::net::TcpListener::bind(format!("{}:{}", cfg.bind, cfg.port))
        .context("bind main port")?;
    http::serve_box(main_l, sup.clone()).context("serve box http")?;

    // Hello: one box per user; the answer lists every project as a workspace.
    let http_client = reqwest::Client::new();
    let listing = loop {
        let hello = http_client
            .post(format!("{}/box/hello", cfg.relay_url))
            .bearer_auth(&cfg.box_token)
            .json(&json!({
                "url": format!("http://127.0.0.1:{}", cfg.port),
                "version": env!("CARGO_PKG_VERSION"),
                "proto": arbos_core::PROTO_VERSION,
                // Baked by image.yml (ARBOS_AGENTD_SHA / ARBOS_IMAGE_VERSION): which commit this
                // box runs, so the relay admin can tell what is shipped.
                "sha": std::env::var("ARBOS_AGENTD_SHA").ok().filter(|v| !v.is_empty() && v != "unknown"),
                "image": std::env::var("ARBOS_IMAGE_VERSION").ok().filter(|v| !v.is_empty() && v != "unknown"),
                // The pi-ext seed this box actually copies into its projects (#1708): on a host
                // sandbox /etc is persisted, so it can lag behind the image.
                "seed_sha": std::fs::read_to_string("/etc/arbos/pi-seed/extensions/arbos-pi-ext/.source-sha")
                    .ok()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty()),
            }))
            .send()
            .await;
        match hello {
            Ok(r) if r.status().is_success() => break r.json::<Value>().await.unwrap_or_default(),
            Ok(r) if r.status() == reqwest::StatusCode::CONFLICT => {
                let text = r.text().await.unwrap_or_default();
                anyhow::bail!(
                    "relay refused this agentd: {}",
                    crate::relay_error::message(&text)
                )
            }
            Ok(r) => tracing::warn!(status = %r.status(), "box hello refused; retrying"),
            Err(e) => tracing::warn!(error = %e, "box hello failed; retrying"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    let user_id = listing
        .get("user_id")
        .and_then(Value::as_str)
        .unwrap_or("u_default")
        .to_owned();
    // Calls Phase 2: the relay's voice link (tool calls and transcript rows from the voice worker).
    voice::spawn_voice_link(sup.clone());
    // The first listing starts every workspace the relay named, and on a box with many projects
    // that is minutes of work (2026-11-09). Nothing below needs it to have finished — the
    // registry poll, the room ceiling and the keep-alive all read the workspace set as it
    // stands — so it runs on its own task and the box says `initializing` until it lands.
    // `apply_listing` takes at most one pass at a time, so the 3 s poll beside it leaves this
    // one alone.
    {
        let sup = sup.clone();
        let listing = listing.clone();
        tokio::spawn(async move {
            sup.apply_listing(&listing).await;
            let n = sup.list().await.len();
            tracing::info!(
                user = %user_id,
                workspaces = n,
                ms = t_boot.elapsed().as_millis() as i64,
                "arbos-agentd booted"
            );
            if let Some(w) = sup.list().await.first() {
                let _ = w
                    .agent
                    .post_timing("agentd_boot_ms", t_boot.elapsed().as_millis() as i64, None)
                    .await;
            }
        });
    }
    // The read-only copy of Arbos's own source (/opt/arbos/src) follows main: refreshed now, in
    // the background, and every few hours after.
    selfref::spawn_refresh();

    // Registry poll (3 s): new projects become workspaces, closed ones stop, settings changes
    // (SetModel, rename, new chats) reach each workspace's pi and doc.
    {
        let sup2 = sup.clone();
        let cfg2 = cfg.clone();
        tokio::spawn(async move {
            let mut poll_failures: u32 = 0;
            // The client itself carries the bounds (connect and whole-request), not only the
            // per-call wrapper: a response that hangs after its headers is what stalled the
            // pass for 9 minutes (#2127); the pool is pinned per connection to one request.
            let poll_client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(crate::workspace::REGISTRY_CALL_TIMEOUT)
                .pool_max_idle_per_host(1)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                // A failed listing names itself once per streak (#2127: a 9-minute gap with
                // no line anywhere), and is bounded so a hang cannot hold the next pass.
                let r = match poll_client
                    .get(format!("{}/box/projects", cfg2.relay_url))
                    .bearer_auth(&cfg2.box_token)
                    .timeout(crate::workspace::REGISTRY_CALL_TIMEOUT)
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        poll_failures += 1;
                        if poll_failures == 1 {
                            tracing::warn!(error = %e, "registry poll: request failed");
                        }
                        continue;
                    }
                };
                if !r.status().is_success() {
                    poll_failures += 1;
                    if poll_failures == 1 {
                        tracing::warn!(status = %r.status(), "registry poll: relay refused the listing");
                    }
                    continue;
                }
                let v = match r.json::<Value>().await {
                    Ok(v) => v,
                    Err(e) => {
                        poll_failures += 1;
                        if poll_failures == 1 {
                            tracing::warn!(error = %e, "registry poll: bad listing");
                        }
                        continue;
                    }
                };
                if poll_failures > 0 {
                    tracing::info!(failures = poll_failures, "registry poll: listing back");
                    poll_failures = 0;
                }
                sup2.apply_listing(&v).await;
            }
        });
    }
    // Box-wide room ceiling (30 s): the per-workspace cap bounds each hub's rooms and nothing
    // bounds their sum, so this is the only pass that can see a box over its CPU quota with
    // every workspace inside its own cap. A box under the ceiling pays one cheap count for it.
    //
    // A sweep rather than a check on the open path, because the sum moves for reasons no single
    // open knows about: a workspace being added puts the box over without any room opening.
    {
        let sup2 = sup.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                sup2.enforce_box_room_ceiling().await;
            }
        });
    }
    // Idle keep-alive heartbeat (turn_running=false lets the relay restore auto-stop).
    {
        let sup2 = sup.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let running = sup2.any_turn_running().await;
                let _ = sup2.keepalive(running).await;
            }
        });
    }
    // Shutdown: SIGTERM → every workspace persists docs + checkpoints and stops its children.
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("sigterm");
    term.recv().await;
    tracing::info!("SIGTERM: stopping workspaces and persisting docs");
    let _ = tokio::time::timeout(Duration::from_secs(45), sup.shutdown()).await;
    std::process::exit(0);
}

#[cfg(test)]
mod relay_url_tests {
    use super::*;
    use arbos_core::relay_url;
    use ed25519_dalek::{Signer, SigningKey};

    const BOX_ID: &str = "user:alice";

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[3u8; 32])
    }

    fn sig_json(
        k: &SigningKey,
        url: &str,
        box_id: &str,
        counter: i64,
        expires_at_ms: i64,
    ) -> String {
        let msg = relay_url::signing_message(url, box_id, counter, expires_at_ms);
        let signed = SignedRelayUrl {
            url: url.into(),
            box_id: box_id.into(),
            counter,
            expires_at_ms,
            sig: base64::engine::general_purpose::STANDARD
                .encode(k.sign(msg.as_bytes()).to_bytes()),
        };
        serde_json::to_string(&signed).unwrap()
    }

    struct Box_ {
        dir: PathBuf,
    }

    impl Box_ {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "arbos-agentd-relay-url-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        /// The plain address file, as a local process could write it.
        fn write_url(&self, name: &str, url: &str) -> PathBuf {
            let p = self.dir.join(name);
            std::fs::write(&p, format!("{url}\n")).unwrap();
            p
        }

        fn write_sig(&self, name: &str, json: &str) {
            std::fs::write(self.dir.join(format!("{name}.sig")), json).unwrap();
        }
    }

    impl Drop for Box_ {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn source(files: Vec<PathBuf>, env_url: Option<&str>, verify: bool) -> RelayUrlSource {
        RelayUrlSource {
            files,
            env_url: env_url.map(str::to_owned),
            verify_key: verify.then(|| key().verifying_key()),
            box_id: BOX_ID.into(),
        }
    }

    #[test]
    fn a_properly_signed_address_is_accepted() {
        let b = Box_::new("ok");
        let p = b.write_url("relay-url", "https://relay.example");
        b.write_sig(
            "relay-url",
            &sig_json(&key(), "https://relay.example", BOX_ID, 7, i64::MAX),
        );
        let got = source(vec![p], None, true).candidates(0);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].url, "https://relay.example");
        assert_eq!(got[0].signed_counter, Some(7));
    }

    #[test]
    fn an_unsigned_file_is_refused_once_a_key_is_pinned() {
        let b = Box_::new("unsigned");
        // What a local process can do after the mode fix is nothing; this is the belt to that
        // braces — even if it could write the file, with no signature beside it the address is
        // not usable.
        let p = b.write_url("relay-url", "https://evil.example");
        let got = source(vec![p], Some("https://relay.example"), true).candidates(0);
        let urls: Vec<&str> = got.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://relay.example"],
            "only the create-time address should survive; an unsigned file is not authority"
        );
    }

    #[test]
    fn a_signature_lifted_from_another_box_is_refused() {
        let b = Box_::new("otherbox");
        let p = b.write_url("relay-url", "https://relay.example");
        b.write_sig(
            "relay-url",
            &sig_json(&key(), "https://relay.example", "user:bob", 7, i64::MAX),
        );
        assert!(source(vec![p], None, true).candidates(0).is_empty());
    }

    #[test]
    fn a_signature_moved_onto_another_address_is_refused() {
        let b = Box_::new("tampered");
        let p = b.write_url("relay-url", "https://relay.example");
        let mut signed: SignedRelayUrl = serde_json::from_str(&sig_json(
            &key(),
            "https://relay.example",
            BOX_ID,
            7,
            i64::MAX,
        ))
        .unwrap();
        signed.url = "https://evil.example".into();
        b.write_sig("relay-url", &serde_json::to_string(&signed).unwrap());
        assert!(
            source(vec![p], None, true).candidates(0).is_empty(),
            "controlling the address must not be enough; the signature covers it"
        );
    }

    #[test]
    fn a_signature_from_a_different_relay_is_refused() {
        let b = Box_::new("otherkey");
        let p = b.write_url("relay-url", "https://evil.example");
        let other = SigningKey::from_bytes(&[9u8; 32]);
        b.write_sig(
            "relay-url",
            &sig_json(&other, "https://evil.example", BOX_ID, 99, i64::MAX),
        );
        assert!(
            source(vec![p], None, true).candidates(0).is_empty(),
            "a signature is only worth the key it was made with"
        );
    }

    #[test]
    fn an_expired_signature_is_refused_so_an_old_address_cannot_be_replayed_for_ever() {
        let b = Box_::new("expired");
        let p = b.write_url("relay-url", "https://relay.example");
        b.write_sig(
            "relay-url",
            &sig_json(&key(), "https://relay.example", BOX_ID, 7, 1_000),
        );
        assert!(source(vec![p.clone()], None, true)
            .candidates(1_001)
            .is_empty());
        assert_eq!(source(vec![p], None, true).candidates(1_000).len(), 1);
    }

    #[test]
    fn the_newest_signed_address_is_preferred_to_an_older_one() {
        let b = Box_::new("counters");
        let old = b.write_url("old", "https://old.example");
        b.write_sig(
            "old",
            &sig_json(&key(), "https://old.example", BOX_ID, 1, i64::MAX),
        );
        let new = b.write_url("new", "https://new.example");
        b.write_sig(
            "new",
            &sig_json(&key(), "https://new.example", BOX_ID, 2, i64::MAX),
        );
        // Given in the stale-first order the old code would have honoured.
        let got = source(vec![old, new], None, true).candidates(0);
        let urls: Vec<&str> = got.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://new.example", "https://old.example"],
            "a rolled-back address must not be preferred to the newest one the relay signed"
        );
    }

    #[test]
    fn the_create_time_address_is_always_a_candidate_and_never_duplicated() {
        let b = Box_::new("env");
        let p = b.write_url("relay-url", "https://relay.example");
        b.write_sig(
            "relay-url",
            &sig_json(&key(), "https://relay.example", BOX_ID, 7, i64::MAX),
        );
        // Same address from both sources: one candidate, not two.
        let same = source(vec![p.clone()], Some("https://relay.example"), true).candidates(0);
        assert_eq!(same.len(), 1);
        // A different create-time address stays available behind the signed one.
        let both = source(vec![p], Some("https://baked.example"), true).candidates(0);
        let urls: Vec<&str> = both.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(urls, vec!["https://relay.example", "https://baked.example"]);
    }

    #[test]
    fn a_box_with_no_pinned_key_still_reads_the_plain_file() {
        let b = Box_::new("nokey");
        let p = b.write_url("relay-url", "https://relay.example");
        let got = source(vec![p], Some("https://baked.example"), false).candidates(0);
        let urls: Vec<&str> = got.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://relay.example", "https://baked.example"],
            "boxes created before signing existed must keep working"
        );
        assert_eq!(got[0].signed_counter, None);
    }

    #[test]
    fn a_signature_that_is_not_json_is_refused_rather_than_ignored() {
        let b = Box_::new("garbage");
        let p = b.write_url("relay-url", "https://relay.example");
        b.write_sig("relay-url", "{not json");
        assert!(source(vec![p], None, true).candidates(0).is_empty());
    }
}
