//! `arbos-boxd` — the root daemon on a compute host that runs each user's box as a
//! self-managed OCI bundle directly under `runsc`, with no Docker in the path
//! (docs/box-scaling.md §3, §4.1).
//!
//! This is Phase 1: the supervisor on its own. It makes a box's disk and bundle (`ensure`),
//! starts it or restores it from a checkpoint (`wake`), stops it clean or checkpoints it
//! (`sleep`), grows its disk online (`grow`), and reports the host (`status`).
//!
//! Not here, on purpose, because later phases own them: the R2 cold tier (`prefetch`,
//! `archive`, fetching a cold box on wake), the relay's `Provider::Boxd`, the idle supervisor
//! that decides when a box sleeps, and more than one node (mTLS). Nothing calls this daemon
//! yet.
//!
//! It listens on loopback only and every request carries a bearer token; see the README for
//! the operator's steps.

mod boxes;
mod http;
mod oci;
mod sys;

use std::net::Ipv4Addr;
use std::sync::Arc;

use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let env = |key: &str, default: &str| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| default.to_owned())
    };
    let token = http::load_token(&env("ARBOS_BOXD_TOKEN_FILE", "/etc/arbos-boxd/token"))?;
    let port: u16 = env("ARBOS_BOXD_PORT", "7790")
        .parse()
        .context("ARBOS_BOXD_PORT")?;
    let cfg = boxes::Cfg {
        root: env("ARBOS_BOXD_ROOT", "/var/lib/arbos-boxd"),
        run: env("ARBOS_BOXD_RUNSC_ROOT", "/var/run/arbos-runsc"),
        platform: env("ARBOS_BOXD_PLATFORM", "systrap"),
    };
    let boxd = Arc::new(boxes::Boxd::new(cfg, Arc::new(sys::Host::new()?)));
    boxd.init().await.context("host setup")?;
    // Loopback, always: the relay is on this host, and a box reaches the host only over the
    // bridge.
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    tracing::info!(port, "arbos-boxd listening on loopback");
    axum::serve(listener, http::router(boxd, &token)).await?;
    Ok(())
}
