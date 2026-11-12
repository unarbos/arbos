//! `arbos-boxd` — the root daemon on a compute host that runs each user's box as a
//! self-managed OCI bundle directly under `runsc`, with no Docker in the path
//! (docs/box-scaling.md §3, §4.1).
//!
//! It makes a box's disk and bundle (`ensure`), starts it or restores it from a checkpoint
//! (`wake`), stops it clean or checkpoints it (`sleep`), grows its disk online (`grow`), moves
//! a sleeping box's checkpoint or disk to an object store and back (`archive`, `prefetch`,
//! and `wake` again; off without `ARBOS_BOXD_S3_ENDPOINT`), and reports the host (`status`).
//!
//! It does what it is asked and decides nothing: when a box sleeps, is archived or is fetched
//! ahead of a wake is the relay's (`arbos-relay/src/boxd.rs`), and so is which host a box is
//! made on.
//!
//! It listens on loopback, and every request carries a bearer token. A host whose relay is
//! elsewhere (docs/box-scaling.md Phase 4) opens a second door with `ARBOS_BOXD_TLS_LISTEN`,
//! which only a holder of a certificate from the operator's CA gets through ([`tls`]). See the
//! README for the operator's steps.

mod boxes;
mod cold;
mod http;
mod oci;
mod sys;
mod tls;

use std::future::IntoFuture;
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
    let subnet = env("ARBOS_BOXD_SUBNET", boxes::SUBNET);
    let tls = tls::from_env()?;
    let cfg = boxes::Cfg {
        root: env("ARBOS_BOXD_ROOT", "/var/lib/arbos-boxd"),
        run: env("ARBOS_BOXD_RUNSC_ROOT", "/var/run/arbos-runsc"),
        platform: env("ARBOS_BOXD_PLATFORM", "systrap"),
        subnet: boxes::parse_subnet(&subnet).with_context(|| {
            format!("ARBOS_BOXD_SUBNET must be a private /16 like 10.208.0.0/16, not {subnet}")
        })?,
        store: cold::Store::from_env()?.map(Arc::new),
    };
    tracing::info!(cold_tier = cfg.store.is_some(), "arbos-boxd starting");
    let boxd = Arc::new(boxes::Boxd::new(cfg, Arc::new(sys::Host::new()?)));
    boxd.init().await.context("host setup")?;
    // Loopback, always: a relay on this host uses it, and a box reaches the host only over the
    // bridge.
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    tracing::info!(port, "arbos-boxd listening on loopback");
    let app = http::router(boxd, &token);
    let plain = axum::serve(listener, app.clone()).into_future();
    let Some(tls) = tls else {
        return Ok(plain.await?);
    };
    let listener = tls.bind().await?;
    tracing::info!(addr = %listener.local, "arbos-boxd listening for a relay with a certificate");
    tokio::try_join!(plain, axum::serve(listener, app).into_future())?;
    Ok(())
}
