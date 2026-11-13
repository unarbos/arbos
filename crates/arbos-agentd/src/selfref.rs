//! Self-reference checkout (docs/agent-self-reference-repo.md). The image bakes a read-only
//! shallow clone of the public unarbos/arbos tree at `/opt/arbos/src`, so an agent asked how
//! Arbos works (encryption, privacy, where data goes) reads the real code and docs instead of
//! guessing. agentd runs as root and refreshes the clone to main's tip at boot and every few
//! hours where the rootfs is writable (Daytona boxes); on a read-only rootfs (D89 host
//! sandboxes) the copy is the image's commit, renewed by every image roll. The model's children
//! (workspace users) can only read it. It is not a path for shipping changes: the clone is reset
//! hard on every refresh.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::process::Command;

/// Where the checkout lives; the prompt (`prompts/core.md`) and `image/pi-seed/AGENTS.md`
/// name it.
pub const SRC_DIR: &str = "/opt/arbos/src";
const REMOTE_BRANCH: &str = "main";
const REFRESH_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const STEP_TIMEOUT: Duration = Duration::from_secs(120);

/// Refresh once now, in the background (boot does not wait), then every `REFRESH_EVERY`.
/// Nothing happens on boxes without the checkout (local dev boxes) or when agentd is not root.
pub fn spawn_refresh() {
    if !Path::new(SRC_DIR).join(".git").is_dir() {
        tracing::debug!(
            dir = SRC_DIR,
            "self-reference checkout absent; not refreshing"
        );
        return;
    }
    if nix::unistd::geteuid().as_raw() != 0 {
        tracing::info!(
            dir = SRC_DIR,
            "self-reference checkout: not root, kept as shipped"
        );
        return;
    }
    // Host sandboxes (D89 gVisor) run the image with a read-only rootfs: there the copy is the
    // image's commit until the next image roll, and a fetch could only fail (EROFS).
    if let Err(e) = write_probe() {
        tracing::info!(
            dir = SRC_DIR,
            error = %e,
            "self-reference checkout: rootfs not writable, the image's copy stays until the next roll"
        );
        return;
    }
    tokio::spawn(async move {
        loop {
            match refresh().await {
                Ok(Some((from, to))) => {
                    tracing::info!(%from, %to, "self-reference checkout refreshed to main")
                }
                Ok(None) => tracing::debug!("self-reference checkout already at main's tip"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "self-reference checkout refresh failed; the shipped copy stays"
                ),
            }
            tokio::time::sleep(REFRESH_EVERY).await;
        }
    });
}

/// `ls-remote` first (a few hundred bytes). Only when main moved: shallow fetch (~27 MB, about
/// a second), hard reset, drop the old objects so the clone does not grow, write bits off again
/// for the files the reset created.
async fn refresh() -> Result<Option<(String, String)>> {
    let head = git(&["rev-parse", "HEAD"]).await?;
    let remote = git(&[
        "ls-remote",
        "origin",
        &format!("refs/heads/{REMOTE_BRANCH}"),
    ])
    .await?;
    let tip = remote
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("ls-remote answered nothing for {REMOTE_BRANCH}"))?
        .to_owned();
    if tip == head {
        return Ok(None);
    }
    git(&["fetch", "-q", "--depth", "1", "origin", REMOTE_BRANCH]).await?;
    git(&["reset", "-q", "--hard", "FETCH_HEAD"]).await?;
    git(&["reflog", "expire", "--expire=now", "--all"]).await?;
    git(&["gc", "-q", "--prune=now"]).await?;
    run("chmod", &["-R", "a-w", SRC_DIR]).await?;
    Ok(Some((head, tip)))
}

/// Create and remove one empty file under `.git`; the only failure mode seen is `EROFS`.
fn write_probe() -> Result<()> {
    let probe = Path::new(SRC_DIR).join(".git").join(".arbos-write-probe");
    std::fs::File::create(&probe).with_context(|| format!("create {}", probe.display()))?;
    std::fs::remove_file(&probe).with_context(|| format!("remove {}", probe.display()))?;
    Ok(())
}

async fn git(args: &[&str]) -> Result<String> {
    let mut all = vec!["-C", SRC_DIR];
    all.extend_from_slice(args);
    run("git", &all).await
}

async fn run(program: &str, args: &[&str]) -> Result<String> {
    let out = tokio::time::timeout(
        STEP_TIMEOUT,
        Command::new(program)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| anyhow!("{program} {} timed out", args.join(" ")))?
    .with_context(|| format!("spawn {program}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
