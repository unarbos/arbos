//! Box lifecycle: providers (Daytona, local process, local Docker, gVisor sandboxes on the relay
//! host — D89, `fleet.rs`), wake jobs, keep-alive, liveness (design §4.5 Flow 5, §4.6; crdt-sync §8).

use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};

use arbos_core::auth::{hash, mint, TokenKind};
use arbos_core::ids::now_ms;
use arbos_core::wire::BoxState;

use crate::daytona::{CreateSandbox, SandboxState};
use crate::db::Sandbox;
use crate::state::{AppState, CreatingGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Daytona,
    Process,
    Docker,
    /// A gVisor (`runsc`) container on the relay host with a per-user `/work` image (D89).
    Gvisor,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "daytona" => Some(Provider::Daytona),
            "process" => Some(Provider::Process),
            "docker" => Some(Provider::Docker),
            "gvisor" => Some(Provider::Gvisor),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Daytona => "daytona",
            Provider::Process => "process",
            Provider::Docker => "docker",
            Provider::Gvisor => "gvisor",
        }
    }
}

/// Where a user without a box gets one: `ARBOS_DEFAULT_PROVIDER` when set (D89: `gvisor` on the
/// arbos.life host), else Daytona when it is configured, else a local process.
pub fn default_provider(state: &AppState) -> Provider {
    match state
        .cfg
        .default_provider
        .as_deref()
        .and_then(Provider::parse)
    {
        Some(p) => p,
        None if state.daytona.is_some() => Provider::Daytona,
        None => Provider::Process,
    }
}

/// Where THIS user's next box goes: the admin override (`users.box_provider`) when set, else
/// [`default_provider`]. The override exists so a scratch user can get a Daytona box on a host
/// whose default is gVisor (the carry e2e of ARB-339); it never moves an existing box.
pub fn provider_for_user(state: &AppState, user_id: &str) -> Provider {
    state
        .db
        .user_provider(user_id)
        .ok()
        .flatten()
        .as_deref()
        .and_then(Provider::parse)
        .unwrap_or_else(|| default_provider(state))
}

/// Environment every box gets. No provider key is in it (D2). Project-level settings (models,
/// instructions, chats) are not baked in any more: the box learns them from `/box/hello` and the
/// `/box/projects` poll, one workspace per project.
pub(crate) fn box_env(
    state: &AppState,
    user_id: &str,
    box_token: &str,
    mcp_token: &str,
    relay_url: &str,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("ARBOS_USER_ID".into(), user_id.to_owned());
    env.insert("ARBOS_RELAY_URL".into(), relay_url.to_owned());
    env.insert("ARBOS_BOX_TOKEN".into(), box_token.to_owned());
    env.insert("ARBOS_MCP_TOKEN".into(), mcp_token.to_owned());
    env.insert("PI_TELEMETRY".into(), "0".into());
    env.insert("PI_SKIP_VERSION_CHECK".into(), "1".into());
    env.insert("PRIME_AGENT_TELEMETRY".into(), "0".into());
    env.insert("DO_NOT_TRACK".into(), "1".into());
    if let Ok(h) = std::env::var("ARBOS_DEFAULT_HARNESS") {
        env.insert("ARBOS_DEFAULT_HARNESS".into(), h);
    }
    // Code mode (D86): one relay setting switches every new box; agentd defaults to off without it.
    if let Ok(m) = std::env::var("ARBOS_CODE_MODE") {
        env.insert("ARBOS_CODE_MODE".into(), m);
    }
    // Stable prefix (D87): same shape.
    if let Ok(m) = std::env::var("ARBOS_STABLE_PREFIX") {
        env.insert("ARBOS_STABLE_PREFIX".into(), m);
    }
    // Compact memory pack (D77 follow-up): same shape.
    if let Ok(m) = std::env::var("ARBOS_MEMORY_PACK_COMPACT") {
        env.insert("ARBOS_MEMORY_PACK_COMPACT".into(), m);
    }
    // Fast memory walk (D77 follow-up): same shape; `off` restores the three-round walk fleet-wide.
    if let Ok(m) = std::env::var("ARBOS_MEMORY_WALK_FAST") {
        env.insert("ARBOS_MEMORY_WALK_FAST".into(), m);
    }
    let _ = state;
    env
}

/// Create the user's box (one Daytona computer per user; created with the user's first project,
/// D32). `snapshot` overrides the relay default (per-user class).
pub async fn create_box(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    snapshot: Option<&str>,
) -> Result<Sandbox> {
    create_box_row(state, user_id, provider, snapshot, true).await
}

/// A Daytona sandbox for the user whose row is NOT written: the upgrade carries `/work` onto it
/// while the row stays on the source, and writes the returned row itself once the carry is in.
pub async fn create_staged_box(
    state: &AppState,
    user_id: &str,
    snapshot: Option<&str>,
) -> Result<Sandbox> {
    create_box_row(state, user_id, Provider::Daytona, snapshot, false).await
}

async fn create_box_row(
    state: &AppState,
    user_id: &str,
    provider: Provider,
    snapshot: Option<&str>,
    write_row: bool,
) -> Result<Sandbox> {
    if !write_row && provider != Provider::Daytona {
        return Err(anyhow!("only a Daytona box can be created staged"));
    }
    let snapshot_name = snapshot
        .map(str::to_owned)
        .unwrap_or_else(|| state.default_snapshot());
    let box_token = mint(TokenKind::Box);
    let mcp_token = mint(TokenKind::Mcp);
    let replacing = state.db.user_box(user_id)?.and_then(|r| r.sandbox_id);
    let _creating = CreatingGuard::new(&state.restart, user_id);
    let mut sb = Sandbox {
        user_id: user_id.to_owned(),
        provider: provider.as_str().to_owned(),
        sandbox_id: None,
        daytona_state: None,
        device_id: String::new(),
        box_token_hash: hash(&box_token),
        mcp_token: mcp_token.clone(),
        snapshot: None,
        last_started_at: None,
        last_stopped_at: None,
        last_seen_at: None,
        box_url: None,
        preview_token: None,
        local_pid: None,
        migrated_from: Vec::new(),
    };
    // A first box gets its row (and token hash) now, so the box can dial in at once. A row that
    // already names a sandbox (upgrade, repair) is left alone until the new sandbox id is known
    // and the whole new row is written in one go: a restart in between leaves the user on the
    // box they had, never on a row without a sandbox (#2262). The new box's early hellos are
    // refused until then and agentd retries every 3 s.
    if replacing.is_none() && write_row {
        state.db.upsert_box(&sb)?;
    }
    state
        .set_box_state(user_id, BoxState::Waking, Some("creating"))
        .await;
    let t0 = Instant::now();
    match provider {
        Provider::Daytona => {
            let dt = state
                .daytona
                .as_ref()
                .ok_or_else(|| anyhow!("DAYTONA_API_KEY not configured"))?;
            let relay_url = state.cfg.public_url.clone();
            let env = box_env(state, user_id, &box_token, &mcp_token, &relay_url);
            let snapshot_ok = dt.get_snapshot(&snapshot_name).await.is_ok();
            let inline = match (&state.cfg.daytona_dockerfile, snapshot_ok) {
                (Some(p), false) => Some(
                    std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?,
                ),
                _ => None,
            };
            // Unique per sandbox: a retained old sandbox (upgrade fallback) keeps its name, and
            // Daytona refuses a duplicate with 409.
            let suffix = arbos_core::ids::ulid().to_ascii_lowercase();
            let sandbox_name = format!(
                "arbos-{}-{}",
                user_id
                    .to_ascii_lowercase()
                    .replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
                &suffix[suffix.len().saturating_sub(6)..]
            );
            let mut body = CreateSandbox {
                name: Some(sandbox_name.clone()),
                snapshot: if inline.is_none() {
                    Some(snapshot_name.clone())
                } else {
                    None
                },
                cpu: inline.as_ref().map(|_| 4),
                memory: inline.as_ref().map(|_| 8),
                disk: inline.as_ref().map(|_| 40),
                build_info: inline.map(|d| json!({ "dockerfileContent": d })),
                // Always-on (default in GitHub mode): Daytona never stops the box by itself.
                auto_stop_interval: Some(if state.cfg.always_on { 0 } else { 30 }),
                auto_archive_interval: Some(7 * 24 * 60),
                auto_delete_interval: Some(-1),
                ttl_minutes: Some(0),
                ..Default::default()
            };
            for (k, v) in env {
                body.env.insert(k, Value::String(v));
            }
            body.labels
                .insert("arbos.user".into(), Value::String(user_id.to_owned()));
            state.db.begin_box_create(
                user_id,
                &sandbox_name,
                provider.as_str(),
                Some(&snapshot_name),
                replacing.as_deref(),
            )?;
            tracing::info!(user = %user_id, sandbox_name, replacing = ?replacing, snapshot = %snapshot_name, "creating box sandbox");
            let created = match dt.create(&body).await {
                Ok(c) => c,
                Err(e) => {
                    let _ = state.db.end_box_create(user_id);
                    return Err(e);
                }
            };
            sb.sandbox_id = Some(created.id.clone());
            sb.device_id = format!("box:{}", created.id);
            sb.snapshot = Some(snapshot_name.clone());
            sb.daytona_state = Some(format!("{:?}", created.state).to_ascii_lowercase());
            if write_row {
                state.db.upsert_box(&sb)?;
            }
            let _ = state.db.end_box_create(user_id);
            let started = match dt
                .wait_for(&created.id, Duration::from_secs(180), |s| {
                    s.state == SandboxState::Started
                })
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    if !write_row {
                        discard_empty_staged(state, dt, &sb).await;
                    }
                    return Err(e);
                }
            };
            sb.daytona_state = Some(format!("{:?}", started.state).to_ascii_lowercase());
            sb.last_started_at = Some(now_ms());
            if let Ok(p) = dt.preview_url(&created.id, 7788).await {
                sb.box_url = Some(p.url);
                sb.preview_token = Some(p.token);
            }
            if write_row {
                state.db.upsert_box(&sb)?;
            }
            push_relay_url(state, dt, &created.id).await;
            state.db.record_timing(
                &timing_key(user_id),
                "daytona_create_to_started_ms",
                t0.elapsed().as_millis() as i64,
                created.runner_id.as_deref(),
            )?;
        }
        Provider::Process => {
            sb.sandbox_id = Some(format!("local-{user_id}"));
            sb.device_id = format!("box:local-{user_id}");
            state.db.upsert_box(&sb)?;
            save_local_tokens(state, user_id, &box_token, &mcp_token)?;
            start_process(state, user_id, &mut sb, &box_token, &mcp_token).await?;
            state.db.record_timing(
                &timing_key(user_id),
                "local_create_ms",
                t0.elapsed().as_millis() as i64,
                None,
            )?;
        }
        Provider::Docker => {
            let name = format!("arbos-box-{}", user_id.to_ascii_lowercase());
            sb.sandbox_id = Some(name.clone());
            sb.device_id = format!("box:{name}");
            state.db.upsert_box(&sb)?;
            save_local_tokens(state, user_id, &box_token, &mcp_token)?;
            start_docker(state, user_id, &mut sb, &box_token, &mcp_token, true).await?;
            state.db.record_timing(
                &timing_key(user_id),
                "docker_create_ms",
                t0.elapsed().as_millis() as i64,
                None,
            )?;
        }
        Provider::Gvisor => {
            if let Err(e) =
                crate::fleet::create_box(state, user_id, &mut sb, &box_token, &mcp_token).await
            {
                // A half-made sandbox must not count as a resident: the next attempt starts clean.
                if replacing.is_none() {
                    let _ = state.db.delete_box_row(user_id);
                }
                return Err(e);
            }
            state.db.record_timing(
                &timing_key(user_id),
                "gvisor_create_ms",
                t0.elapsed().as_millis() as i64,
                None,
            )?;
        }
    }
    state
        .wake_started
        .lock()
        .await
        .insert(user_id.to_owned(), now_ms());
    Ok(sb)
}

/// Delete a staged sandbox that never became the user's box and holds no user data. When Daytona
/// refuses (a 502, a state in transition) it is recorded in `box_history`, due now, so the hourly
/// sweeper retries the delete instead of leaving it running with no record.
pub async fn discard_empty_staged(
    state: &AppState,
    dt: &crate::daytona::Daytona,
    staged: &Sandbox,
) {
    let Some(id) = staged.sandbox_id.as_deref() else {
        return;
    };
    let _ = dt.stop(id, true).await;
    match dt.delete(id).await {
        Ok(_) => tracing::info!(user = %staged.user_id, sandbox = %id, "empty staged box deleted"),
        Err(e) => {
            tracing::warn!(user = %staged.user_id, sandbox = %id, error = %e, "empty staged box delete failed; the history sweeper retries it");
            if let Err(e) = state.db.insert_partial_box_history(staged, now_ms()) {
                tracing::error!(user = %staged.user_id, sandbox = %id, error = %e, "could not record the staged box for the sweeper");
            }
        }
    }
}

/// Box-level timings are recorded under the user key (`GET /projects/{id}/timings` merges them in).
pub fn timing_key(user_id: &str) -> String {
    format!("user:{user_id}")
}

/// Four consecutive free loopback ports for a process box (main, MCP, hook, model stub), chosen
/// by binding rather than hashing the project id (ULID digits w–z are not base-32 digits).
async fn allocate_local_ports(state: &AppState) -> Result<u16> {
    let taken: std::collections::HashSet<u16> = state
        .db
        .list_boxes()?
        .into_iter()
        .filter_map(|s| {
            s.box_url
                .and_then(|u| u.rsplit(':').next().and_then(|p| p.parse().ok()))
        })
        .collect();
    let mut base = 17_000u16;
    while base < 60_000 {
        if (0..4).all(|i| {
            !taken.contains(&(base + i))
                && std::net::TcpListener::bind(("127.0.0.1", base + i)).is_ok()
        }) {
            return Ok(base);
        }
        base += 4;
    }
    Err(anyhow!("no free local port range"))
}

async fn start_process(
    state: &AppState,
    user_id: &str,
    sb: &mut Sandbox,
    box_token: &str,
    mcp_token: &str,
) -> Result<()> {
    let work = state.cfg.data_dir.join("boxes").join(user_id).join("work");
    std::fs::create_dir_all(&work)?;
    // A process box that is still alive (relay restart, double wake) must go before a new one binds.
    if let Some(mut child) = state.local_children.lock().await.remove(user_id) {
        if let Some(pid) = child.id() {
            let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
        }
        let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
    } else if let Some(pid) = sb.local_pid {
        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let port = allocate_local_ports(state).await?;
    let env = box_env(state, user_id, box_token, mcp_token, &state.cfg.public_url);
    let mut cmd = tokio::process::Command::new(&state.cfg.agentd_bin);
    // D2 also holds for a local box: start from an empty environment and pass an allow-list, so the
    // relay's provider and Daytona keys never reach agentd or its children.
    cmd.env_clear();
    for (k, v) in std::env::vars() {
        let pass = matches!(
            k.as_str(),
            "PATH" | "HOME" | "LANG" | "LC_ALL" | "TERM" | "TMPDIR" | "RUST_LOG" | "RUST_BACKTRACE"
        ) || ((k.starts_with("ARBOS_")
            || k.starts_with("PRIME_AGENT_")
            || k.starts_with("PI_"))
            && !k.contains("KEY")
            && !k.contains("TOKEN"));
        if pass {
            cmd.env(k, v);
        }
    }
    cmd.arg("boot")
        .envs(env)
        .env("ARBOS_STATE_DIR", work.join(".arbos"))
        .env("ARBOS_WORK_DIR", &work)
        .env("ARBOS_PORT", port.to_string())
        .env("ARBOS_BIND", "127.0.0.1")
        .env("ARBOS_LOCAL_BOX", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::fs::File::create(
            work.join("agentd.out.log"),
        )?))
        .stderr(Stdio::from(std::fs::File::create(
            work.join("agentd.err.log"),
        )?))
        .kill_on_drop(false);
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", state.cfg.agentd_bin.display()))?;
    sb.local_pid = child.id().map(i64::from);
    sb.box_url = Some(format!("http://127.0.0.1:{port}"));
    sb.last_started_at = Some(now_ms());
    state.db.upsert_box(sb)?;
    // The relay URL file for a local box: same contract as Daytona, written straight to disk.
    let _ = std::fs::create_dir_all(work.join(".arbos"));
    let _ = std::fs::write(
        work.join(".arbos").join("relay-url"),
        format!("{}\n", state.cfg.public_url.trim_end_matches('/')),
    );
    state
        .local_children
        .lock()
        .await
        .insert(user_id.to_owned(), child);
    Ok(())
}

async fn start_docker(
    state: &AppState,
    user_id: &str,
    sb: &mut Sandbox,
    box_token: &str,
    mcp_token: &str,
    create: bool,
) -> Result<()> {
    let name = sb.sandbox_id.clone().unwrap_or_default();
    if create {
        let relay_url = state
            .cfg
            .public_url
            .replace("127.0.0.1", "host.docker.internal")
            .replace("localhost", "host.docker.internal");
        let env = box_env(state, user_id, box_token, mcp_token, &relay_url);
        let mut cmd = tokio::process::Command::new("docker");
        cmd.args([
            "run",
            "-d",
            "--name",
            &name,
            "--add-host",
            "host.docker.internal:host-gateway",
            "-p",
            "127.0.0.1::7788",
        ]);
        for (k, v) in env {
            cmd.arg("-e").arg(format!("{k}={v}"));
        }
        cmd.arg(&state.cfg.local_box_image);
        let out = cmd.output().await.context("docker run")?;
        if !out.status.success() {
            return Err(anyhow!(
                "docker run failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    } else {
        let out = tokio::process::Command::new("docker")
            .args(["start", &name])
            .output()
            .await
            .context("docker start")?;
        if !out.status.success() {
            return Err(anyhow!(
                "docker start failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    let out = tokio::process::Command::new("docker")
        .args(["port", &name, "7788/tcp"])
        .output()
        .await?;
    let port_line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    let port = port_line.rsplit(':').next().unwrap_or("7788").to_owned();
    sb.box_url = Some(format!("http://127.0.0.1:{port}"));
    sb.last_started_at = Some(now_ms());
    state.db.upsert_box(sb)?;
    Ok(())
}

/// Start (wake) an existing box. Returns once the start call is accepted; the box's own
/// room dial marks it awake.
pub async fn start_box(state: &AppState, user_id: &str) -> Result<()> {
    // No users row ⇒ never real: a stale box row (phantom minted for a project id) must not
    // get a container back, whatever asked for it.
    if state.db.user(user_id)?.is_none() {
        let _ = state.db.finish_wake_job(user_id);
        state
            .set_box_state(user_id, BoxState::Error, Some("no such user"))
            .await;
        return Err(anyhow!(
            "start_box: {user_id} has no users row; not creating a box"
        ));
    }
    let mut sb = match state.db.user_box(user_id)? {
        Some(sb) => sb,
        None => {
            // A user with projects but no box yet (pre-D70 data, or a fresh sign-in): create it on
            // first wake. On a full host (D89) the user is waitlisted instead and this is not an
            // error: the admission tick wakes them later.
            let provider = provider_for_user(state, user_id);
            tracing::info!(%user_id, provider = provider.as_str(), "no box for this user yet; creating one");
            match ensure_user_box(state, user_id, provider).await {
                Ok(_) => {}
                Err(e) if e.to_string() == crate::fleet::WAITLISTED => {
                    let _ = state.db.finish_wake_job(user_id);
                    state
                        .set_box_state(user_id, BoxState::Asleep, Some("waitlisted"))
                        .await;
                }
                Err(e) => return Err(e),
            }
            return Ok(());
        }
    };
    let provider = Provider::parse(&sb.provider).ok_or_else(|| anyhow!("bad provider"))?;
    state
        .wake_started
        .lock()
        .await
        .insert(user_id.to_owned(), now_ms());
    state
        .set_box_state(user_id, BoxState::Waking, Some("starting"))
        .await;
    // The box token is minted once per box (Daytona env is fixed at create). Local providers re-inject it
    // from tokens.json; the relay DB holds only the hash.
    let tokens = load_local_tokens(state, user_id)?;
    match provider {
        Provider::Daytona => {
            let dt = state
                .daytona
                .as_ref()
                .ok_or_else(|| anyhow!("DAYTONA_API_KEY not configured"))?;
            let id = sb
                .sandbox_id
                .clone()
                .ok_or_else(|| anyhow!("no sandbox id"))?;
            let t0 = Instant::now();
            let cur = dt.get(&id).await?;
            let from = cur.state;
            if cur.state == SandboxState::Started {
                // The sandbox runs but agentd is gone (Daytona's daemon does not restart a dead
                // entrypoint): bounce it so the entrypoint session starts again.
                tracing::warn!(%user_id, "sandbox is started but the box is silent; restarting it");
                dt.stop(&id, true).await?;
                dt.wait_for(&id, Duration::from_secs(120), |s| {
                    s.state == SandboxState::Stopped
                })
                .await?;
            }
            dt.start(&id).await?;
            let s = dt
                .wait_for(&id, Duration::from_secs(180), |s| {
                    s.state == SandboxState::Started
                })
                .await?;
            sb.daytona_state = Some(format!("{:?}", s.state).to_ascii_lowercase());
            if let Ok(p) = dt.preview_url(&id, 7788).await {
                sb.box_url = Some(p.url);
                sb.preview_token = Some(p.token);
            }
            sb.last_started_at = Some(now_ms());
            if !state.db.write_back_box(&sb, Some(&id))? {
                // The row moved on while this start ran (an upgrade swapped or rolled back): the
                // sandbox just started is not the user's box, so it goes back to sleep.
                tracing::warn!(%user_id, sandbox = %id, "box row moved to another sandbox while this start ran; not writing it back");
                let _ = dt.stop(&id, false).await;
                return Err(anyhow!(
                    "the box row moved to another sandbox while {id} was starting"
                ));
            }
            state.fresh_preview_tokens.lock().await.remove(&id);
            if state.cfg.always_on {
                // Boxes created before always-on still carry a 30-minute autostop.
                let _ = dt.set_autostop(&id, 0).await;
            }
            state.db.record_timing(
                &timing_key(user_id),
                &format!("daytona_start_from_{:?}_ms", from).to_ascii_lowercase(),
                t0.elapsed().as_millis() as i64,
                None,
            )?;
            push_relay_url(state, dt, &id).await;
        }
        Provider::Process => {
            let (box_token, mcp_token) = tokens.ok_or_else(|| anyhow!("local tokens missing"))?;
            start_process(state, user_id, &mut sb, &box_token, &mcp_token).await?;
        }
        Provider::Docker => {
            let (box_token, mcp_token) = tokens.ok_or_else(|| anyhow!("local tokens missing"))?;
            start_docker(state, user_id, &mut sb, &box_token, &mcp_token, false).await?;
        }
        Provider::Gvisor => {
            let t0 = Instant::now();
            crate::fleet::start_box(state, user_id, &mut sb, tokens).await?;
            state.db.record_timing(
                &timing_key(user_id),
                "gvisor_start_ms",
                t0.elapsed().as_millis() as i64,
                None,
            )?;
        }
    }
    Ok(())
}

/// Stop (hibernate) a box. `force` is the Stop-project path (§4.7).
pub async fn stop_box(state: &AppState, user_id: &str, force: bool) -> Result<()> {
    let mut sb = state
        .db
        .user_box(user_id)?
        .ok_or_else(|| anyhow!("no box for user {user_id}"))?;
    let provider = Provider::parse(&sb.provider).ok_or_else(|| anyhow!("bad provider"))?;
    match provider {
        Provider::Daytona => {
            let dt = state
                .daytona
                .as_ref()
                .ok_or_else(|| anyhow!("DAYTONA_API_KEY not configured"))?;
            let id = sb
                .sandbox_id
                .clone()
                .ok_or_else(|| anyhow!("no sandbox id"))?;
            let t0 = Instant::now();
            dt.stop(&id, force).await?;
            if force {
                let _ = dt.update_secrets(&id, vec![]).await;
            }
            let mut s = dt
                .wait_for(&id, Duration::from_secs(120), |s| {
                    matches!(s.state, SandboxState::Stopped | SandboxState::Archived)
                })
                .await?;
            if user_id.starts_with("test_") && matches!(s.state, SandboxState::Stopped) {
                // Test users' boxes (rigs, evals, demo accounts) do not need a warm restart: archive
                // them off the disk quota as soon as they stop; `start` restores an archived sandbox.
                match dt.archive(&id).await {
                    Ok(a) => s = a,
                    Err(e) => {
                        tracing::warn!(user = %user_id, sandbox = %id, error = %e, "archive of a stopped test box")
                    }
                }
            }
            sb.daytona_state = Some(format!("{:?}", s.state).to_ascii_lowercase());
            state.db.record_timing(
                &timing_key(user_id),
                "daytona_stop_ms",
                t0.elapsed().as_millis() as i64,
                None,
            )?;
        }
        Provider::Process => {
            if let Some(mut child) = state.local_children.lock().await.remove(user_id) {
                if let Some(pid) = child.id() {
                    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
                }
                // One agentd now stops every workspace's children; give it the full grace.
                if tokio::time::timeout(Duration::from_secs(40), child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                }
            } else if let Some(pid) = sb.local_pid {
                let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
            }
            sb.local_pid = None;
        }
        Provider::Docker => {
            let name = sb.sandbox_id.clone().unwrap_or_default();
            let _ = tokio::process::Command::new("docker")
                .args(["stop", "-t", "10", &name])
                .output()
                .await;
        }
        Provider::Gvisor => {
            let name = sb.sandbox_id.clone().unwrap_or_default();
            // One agentd stops every workspace's children; give it the full grace.
            crate::fleet::stop_box(&name, 30).await?;
        }
    }
    sb.last_stopped_at = Some(now_ms());
    if !state.db.write_back_box(&sb, sb.sandbox_id.as_deref())? {
        tracing::warn!(%user_id, sandbox = ?sb.sandbox_id, "box row moved to another sandbox while this stop ran; not writing it back");
        return Ok(());
    }
    state.box_conns.lock().await.remove(user_id);
    state
        .set_box_state(
            user_id,
            BoxState::Asleep,
            Some(if force { "stopped_by_user" } else { "stopped" }),
        )
        .await;
    if force {
        for p in state.db.projects_of_user(user_id).unwrap_or_default() {
            let _ = state.db.set_project_state(&p.id, "stopped_by_user");
        }
    }
    Ok(())
}

/// Local providers keep the plaintext box/MCP tokens in the relay data dir so a restart can re-inject them.
pub fn save_local_tokens(
    state: &AppState,
    user_id: &str,
    box_token: &str,
    mcp_token: &str,
) -> Result<()> {
    let dir = state.cfg.data_dir.join("boxes").join(user_id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("tokens.json"),
        json!({ "box": box_token, "mcp": mcp_token }).to_string(),
    )?;
    Ok(())
}

pub fn load_local_tokens(state: &AppState, user_id: &str) -> Result<Option<(String, String)>> {
    let p = state
        .cfg
        .data_dir
        .join("boxes")
        .join(user_id)
        .join("tokens.json");
    if !p.exists() {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p)?)?;
    Ok(Some((
        v.get("box")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        v.get("mcp")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    )))
}

/// A client needs the box: queue a wake obligation unless it is already awake or waking.
/// The sandbox env bakes `ARBOS_RELAY_URL` at create time; the relay's public URL can change
/// (quick tunnels do on every restart). After every start the current URL goes into the box as
/// `/home/daytona/.arbos/relay-url` (and the legacy `/work/.arbos/relay-url` when that directory
/// exists), which agentd prefers over its env at boot. Written with `sudo` through the toolbox
/// shell: the plain toolbox upload runs as the sandbox user and failed with "permission denied"
/// both under the root-owned `/work` (D70) and against a root-written file in a sticky directory.
pub const RELAY_URL_FILE: &str = "/home/daytona/.arbos/relay-url";

async fn push_relay_url(state: &AppState, dt: &crate::daytona::Daytona, sandbox_id: &str) {
    let url = state.cfg.public_url.trim_end_matches('/').to_owned();
    // Written through a root shell rather than the toolbox upload: the upload runs as the
    // sandbox user and cannot replace a file agentd (root) wrote. The file ends up owned by the
    // sandbox user, world-readable, in a non-sticky directory, so either side can rewrite it.
    let dir = std::path::Path::new(RELAY_URL_FILE)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/home/daytona/.arbos".to_owned());
    let legacy_dir = "/work/.arbos";
    let cmd = format!(
        "sudo -n sh -c 'mkdir -p {dir} && printf \"%s\\n\" \"{url}\" > {file} && chown daytona:daytona {dir} {file} 2>/dev/null; chmod 0777 {dir}; chmod 0666 {file}; \
         if [ -d {legacy_dir} ]; then printf \"%s\\n\" \"{url}\" > {legacy_dir}/relay-url 2>/dev/null && chmod 0666 {legacy_dir}/relay-url 2>/dev/null; fi; echo ok'",
        file = RELAY_URL_FILE,
    );
    for attempt in 1..=4u32 {
        match dt.toolbox_exec(sandbox_id, &cmd, 30).await {
            Ok(out) if out.get("exitCode").and_then(Value::as_i64).unwrap_or(1) == 0 => {
                tracing::info!(sandbox_id, %url, attempt, "relay url pushed into the box");
                return;
            }
            Ok(out) => {
                tracing::warn!(sandbox_id, attempt, result = ?out.get("result"), "relay url push failed");
            }
            Err(e) => {
                tracing::warn!(sandbox_id, attempt, error = %e, "relay url push failed");
            }
        }
        tokio::time::sleep(Duration::from_millis(1500 * attempt as u64)).await;
    }
}

/// A started box must say hello within this long, or it is treated as unable to reach the relay.
pub const HELLO_DEADLINE_MS: i64 = 60_000;
/// A wake timer older than this is stale and is not recorded as `wake_to_box_connected_ms`.
const STALE_WAKE_TIMER_MS: i64 = 5 * 60_000;
/// Reasons that count as the user asking again (they reset a `box cannot reach relay` error).
fn explicit_wake(reason: &str) -> bool {
    matches!(reason, "wake" | "start" | "open" | "api")
}

/// `ensure_awake` for the box that serves a project.
pub async fn ensure_awake_project(state: &AppState, project_id: &str, reason: &str) {
    if let Some(u) = state.user_of(project_id) {
        ensure_awake(state, &u, reason).await;
    }
}

/// An agent that serves a chat room: the project whose push queue reaches it and the user
/// whose box runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingAgent {
    pub project_id: String,
    pub owner: String,
}

/// Push kinds that tell a box to open one chat's room now (#2120; agentd handles them from
/// 0.1.169 — older agentd logs them as unknown). `chat_open` names a project chat
/// (`{chat_id, reason, project_id, title?, is_pinned?, created_at?, forked_from?}`);
/// `people_chat_open` a people chat (`{chat_id, reason}`). Neither marks a late join on the
/// box: the rows waiting in the room are the work.
pub const CHAT_OPEN_PUSH: &str = "chat_open";
pub const PEOPLE_CHAT_OPEN_PUSH: &str = "people_chat_open";

/// The `chat_open` payload: the chat row rides along because agentd's `open_listed_chat`
/// upserts title / pin / created_at from it (a bare id would reset them).
fn chat_open_payload(chat: &crate::db::Chat, reason: &str) -> Value {
    json!({
        "chat_id": chat.id,
        "project_id": chat.project_id,
        "reason": reason,
        "title": chat.title,
        "is_pinned": chat.is_pinned,
        "created_at": chat.created_at,
        "forked_from": chat.forked_from,
    })
}

/// A new project chat exists: its box opens the room without waiting for the 3 s listing.
/// Returns whether a push was queued (false when one is already waiting).
pub fn chat_created(state: &AppState, chat: &crate::db::Chat) -> bool {
    match state.db.enqueue_push_once(
        &chat.project_id,
        CHAT_OPEN_PUSH,
        &chat_open_payload(chat, "created"),
    ) {
        Ok(added) => added,
        Err(e) => {
            tracing::warn!(chat = %chat.id, project = %chat.project_id, error = %e, "chat_open push not queued");
            false
        }
    }
}

/// A person's row landed in `chat` — over a room socket or through `POST /chats/{id}/messages`:
/// the wake obligation for every serving agent's box (crdt-sync §8.4 steps 1–2), and, for a
/// box with no socket on this room, a push that opens the room now rather than at its next
/// listing (#2120: the first row of a new chat used to reach the box only through the 3 s
/// `/box/projects` poll, and a stalled poll cost Jacob a 9-minute first reply). Returns how
/// many open pushes were queued.
pub async fn after_person_row(
    state: &AppState,
    room: &crate::state::Room,
    chat: &crate::notify::ChatRef,
    agents: &[ServingAgent],
) -> usize {
    let mut queued = 0;
    for a in agents {
        state
            .last_client_push
            .lock()
            .await
            .insert(a.owner.clone(), now_ms());
        ensure_awake(state, &a.owner, "send").await;
        if room.has_box(&a.owner) {
            continue;
        }
        let (kind, payload) = match &chat.project_id {
            None => (
                PEOPLE_CHAT_OPEN_PUSH,
                json!({ "chat_id": chat.chat_id, "reason": "rows_pending" }),
            ),
            Some(pid) => {
                // A row into a chat the relay has no row for (gone meanwhile): nothing to open.
                let Ok(Some(row)) = state.db.chat(pid, &chat.chat_id) else {
                    continue;
                };
                (CHAT_OPEN_PUSH, chat_open_payload(&row, "rows_pending"))
            }
        };
        match state.db.enqueue_push_once(&a.project_id, kind, &payload) {
            Ok(true) => {
                queued += 1;
                tracing::info!(
                    chat = %chat.chat_id, project = %a.project_id, owner = %a.owner, kind,
                    "row landed with no box socket on the room; told the box to open it"
                );
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(chat = %chat.chat_id, project = %a.project_id, error = %e, "open push not queued")
            }
        }
    }
    queued
}

/// The user's box, created on first use (D32 moved from project to user).
pub async fn ensure_user_box(
    state: &AppState,
    user_id: &str,
    provider: Provider,
) -> Result<Sandbox> {
    if let Some(sb) = state.db.user_box(user_id)? {
        return Ok(sb);
    }
    if provider == Provider::Gvisor && !crate::fleet::try_admit(state, user_id).await? {
        return Err(anyhow!(crate::fleet::WAITLISTED));
    }
    let snapshot = state.db.user_snapshot(user_id)?;
    create_box(state, user_id, provider, snapshot.as_deref()).await
}

pub async fn ensure_awake(state: &AppState, user_id: &str, reason: &str) {
    let (st, since) = state.box_state(user_id).await;
    if st == BoxState::Awake {
        // Nothing to wake; a timer left from an earlier attempt would only mislabel the next
        // reconnect as a slow wake.
        state.wake_started.lock().await.remove(user_id);
        return;
    }
    // Only the kill switch keeps a machine down: machine time is outside the credit balance and
    // caps (machines are always on), so an empty inference balance never blocks a wake.
    if let Err(r) = crate::credits::check_read_only(state) {
        tracing::info!(%user_id, %r, "wake refused");
        state
            .set_box_state(user_id, BoxState::Error, Some(&r.to_string()))
            .await;
        return;
    }
    if explicit_wake(reason) {
        let _ = state.db.set_box_held(user_id, false);
    }
    // Evicted under host pressure (D89): only the user's own activity brings it back; the
    // always-on sweep leaves it down so the pressure does not come straight back.
    if crate::fleet::is_evicted(state, user_id).await {
        if reason == "always-on" {
            return;
        }
        crate::fleet::clear_evicted(state, user_id).await;
    }
    if st == BoxState::Error {
        // Stale-box error: no more automatic retries; an explicit wake/start tries again once.
        if !explicit_wake(reason) {
            return;
        }
        state.stale_bounces.lock().await.remove(user_id);
    }
    if st == BoxState::Waking {
        // Already waking: either still inside the hello deadline (nothing to do) or stale, in
        // which case `stale_wake_check` has bounced it once or flipped it to error.
        let _ = stale_wake_check(state, user_id, since).await;
        return;
    }
    if let Err(e) = state.db.upsert_wake_job(user_id, reason) {
        tracing::warn!(error = %e, "wake job");
    }
    state
        .wake_started
        .lock()
        .await
        .insert(user_id.to_owned(), now_ms());
    state
        .set_box_state(user_id, BoxState::Waking, Some(reason))
        .await;
}

/// Stale-box detector (runs from the liveness sweep and from `ensure_awake`): a box that has been
/// waking for longer than `HELLO_DEADLINE_MS` without a socket is re-pushed the relay URL and
/// bounced once; the second time it becomes `error · box cannot reach relay` and the wake job
/// ends, so the relay stops waking it forever. Returns true when it acted.
async fn stale_wake_check(state: &AppState, user_id: &str, since: i64) -> bool {
    if box_reachable(state, user_id).await || now_ms() - since < HELLO_DEADLINE_MS {
        return false;
    }
    // An upgrade stops agentd on purpose while it packs /work (minutes for a multi-GB
    // workspace); bouncing the sandbox underneath it kills the toolbox session mid-pack.
    if state.upgrading.lock().await.contains_key(user_id) {
        return false;
    }
    let bounces = {
        let mut m = state.stale_bounces.lock().await;
        let n = m.entry(user_id.to_owned()).or_insert(0);
        *n += 1;
        *n
    };
    let _ = state.db.finish_wake_job(user_id);
    if bounces > 1 {
        tracing::error!(%user_id, "box started but never reached the relay; giving up");
        state
            .set_box_state(user_id, BoxState::Error, Some("box cannot reach relay"))
            .await;
        return true;
    }
    tracing::warn!(%user_id, "box has been waking for >60 s without connecting; re-pushing the relay url and bouncing once");
    if let Err(e) = state.db.upsert_wake_job(user_id, "stale box: bounce") {
        tracing::warn!(error = %e, "wake job");
    }
    state
        .wake_started
        .lock()
        .await
        .insert(user_id.to_owned(), now_ms());
    // Restart the clock: the next check happens one deadline after the bounce.
    state
        .box_states
        .write()
        .await
        .insert(user_id.to_owned(), (BoxState::Waking, now_ms()));
    true
}

/// A box's HTTP heartbeat is this fresh at most (its registry poll runs every 3 s).
pub const BOX_HTTP_LIVE_MS: i64 = 20_000;

/// Is the box there: a room socket open, or an HTTP call from it moments ago? A box whose user
/// has no project yet never opens a socket (nothing to sync) but polls `/box/projects` all the
/// same, so the poll is its heartbeat.
pub async fn box_reachable(state: &AppState, user_id: &str) -> bool {
    let sockets = state
        .box_conns
        .lock()
        .await
        .get(user_id)
        .map(|c| c.0 > 0)
        .unwrap_or(false);
    if sockets {
        return true;
    }
    state
        .box_http_seen
        .lock()
        .await
        .get(user_id)
        .is_some_and(|t| now_ms() - t < BOX_HTTP_LIVE_MS)
}

/// `/box/hello`, `/box/projects`, `/box/keepalive`: the box is alive, socket or not. A box the
/// relay still believed waking (or asleep, or given up on) is awake from this moment.
pub async fn box_seen_http(state: &AppState, user_id: &str) {
    state
        .box_http_seen
        .lock()
        .await
        .insert(user_id.to_owned(), now_ms());
    let (st, _) = state.box_state(user_id).await;
    if st != BoxState::Awake {
        tracing::info!(%user_id, from = ?st, "box reached the relay over HTTP; awake");
        box_connected_inner(state, user_id).await;
    }
}

pub async fn box_connected(state: &AppState, user_id: &str) {
    {
        let mut c = state.box_conns.lock().await;
        let e = c.entry(user_id.to_owned()).or_insert((0, 0));
        e.0 += 1;
    }
    box_connected_inner(state, user_id).await;
}

/// What a connection and an HTTP heartbeat have in common: the wake is over, the box is awake.
async fn box_connected_inner(state: &AppState, user_id: &str) {
    state.stale_bounces.lock().await.remove(user_id);
    let _ = state.db.finish_wake_job(user_id);
    if let Some(t0) = state.wake_started.lock().await.remove(user_id) {
        // A timer older than the hello deadline belongs to an abandoned wake (the box was
        // already awake, or a wake job gave up); it is not a measurement.
        let elapsed = now_ms() - t0;
        if elapsed <= STALE_WAKE_TIMER_MS {
            let _ = state
                .db
                .record_timing(user_id, "wake_to_box_connected_ms", elapsed, None);
        } else {
            tracing::debug!(%user_id, elapsed, "stale wake timer dropped");
        }
    }
    state.set_box_state(user_id, BoxState::Awake, None).await;
}

pub async fn box_disconnected(state: &AppState, user_id: &str) {
    let mut c = state.box_conns.lock().await;
    let e = c.entry(user_id.to_owned()).or_insert((0, 0));
    e.0 = e.0.saturating_sub(1);
    e.1 = now_ms();
}

/// Called every 5 s: a box with no sockets for `box_idle_grace_ms` is asleep (unless it was never seen),
/// and a box waking past the hello deadline goes through the stale-box detector.
pub async fn sweep_box_liveness(state: &AppState) {
    let waking: Vec<(String, i64)> = state
        .box_states
        .read()
        .await
        .iter()
        .filter(|(_, (st, _))| *st == BoxState::Waking)
        .map(|(k, (_, since))| (k.clone(), *since))
        .collect();
    for (user_id, since) in waking {
        stale_wake_check(state, &user_id, since).await;
    }
    let snapshot: Vec<(String, u32, i64)> = state
        .box_conns
        .lock()
        .await
        .iter()
        .map(|(k, (n, t))| (k.clone(), *n, *t))
        .collect();
    // Boxes that never opened a socket (no project yet) are alive only through their HTTP
    // heartbeat; when that stops for the grace period they are asleep like any other.
    let silent: Vec<String> = {
        let seen = state.box_http_seen.lock().await;
        let conns = state.box_conns.lock().await;
        seen.iter()
            .filter(|(u, t)| {
                now_ms() - **t > state.cfg.box_idle_grace_ms.max(BOX_HTTP_LIVE_MS)
                    && conns.get(*u).map(|c| c.0).unwrap_or(0) == 0
            })
            .map(|(u, _)| u.clone())
            .collect()
    };
    for user_id in silent {
        let (st, _) = state.box_state(&user_id).await;
        let had_socket = state
            .box_conns
            .lock()
            .await
            .get(&user_id)
            .is_some_and(|c| c.1 > 0);
        if st == BoxState::Awake && !had_socket {
            state
                .set_box_state(&user_id, BoxState::Asleep, Some("box heartbeat lost"))
                .await;
        }
        state.box_http_seen.lock().await.remove(&user_id);
    }
    for (user_id, n, last) in snapshot {
        if n == 0 && last > 0 && now_ms() - last > state.cfg.box_idle_grace_ms {
            let (st, _) = state.box_state(&user_id).await;
            if st == BoxState::Awake {
                state
                    .set_box_state(&user_id, BoxState::Asleep, Some("box socket lost"))
                    .await;
                // A client row that landed while the box was already dying (still "awake" in the
                // grace window) has no box to take it: wake again so the command is not stranded.
                let pushed = state
                    .last_client_push
                    .lock()
                    .await
                    .get(&user_id)
                    .copied()
                    .unwrap_or(0);
                if pushed >= last - 5_000 {
                    tracing::warn!(%user_id, "box died with client rows outstanding; waking it again");
                    ensure_awake(state, &user_id, "rows pending after box loss").await;
                }
            }
        }
    }
}

/// Safe project-id path segment: ulids and similar ids only (no `/`, `..`, or whitespace).
fn safe_work_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The preview token to send to this box: one learned after a 401 wins over the row's.
pub(crate) async fn preview_token_for(state: &AppState, sb: &Sandbox) -> Option<String> {
    if let Some(id) = &sb.sandbox_id {
        if let Some(t) = state.fresh_preview_tokens.lock().await.get(id) {
            return Some(t.clone());
        }
    }
    sb.preview_token.clone()
}

/// Daytona's current preview token for the box when it differs from `used`, remembered in memory.
async fn fresh_preview_token(state: &AppState, sb: &Sandbox, used: Option<&str>) -> Option<String> {
    if sb.provider != Provider::Daytona.as_str() {
        return None;
    }
    let (dt, id) = (state.daytona.as_ref()?, sb.sandbox_id.as_ref()?);
    let token = match dt.preview_url(id, 7788).await {
        Ok(p) => p.token,
        Err(e) => {
            tracing::warn!(user = %sb.user_id, sandbox = %id, error = %e, "box answered 401; preview token lookup failed");
            return None;
        }
    };
    if used == Some(token.as_str()) {
        return None;
    }
    tracing::warn!(user = %sb.user_id, sandbox = %id, "box answered 401 to a stale preview token; using Daytona's current one");
    state
        .fresh_preview_tokens
        .lock()
        .await
        .insert(id.clone(), token.clone());
    Some(token)
}

/// Send one request to the user's box URL. Daytona gives a sandbox a new preview token every time
/// it stops and starts, and the row learns it only when a start run by the relay finishes. A start
/// that timed out, or one Daytona completed later, leaves the row's token stale and the proxy
/// answers 401 to every call while the box itself is fine (2026-10-29 16:05–16:45, four boxes).
/// On a 401 the current token is read from Daytona and the request is sent once more.
pub(crate) async fn send_to_box(
    state: &AppState,
    sb: &Sandbox,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> reqwest::Result<reqwest::Response> {
    let with = |t: Option<&str>| match t {
        Some(t) => build().header("x-daytona-preview-token", t),
        None => build(),
    };
    let used = preview_token_for(state, sb).await;
    let r = with(used.as_deref()).send().await?;
    if r.status() != reqwest::StatusCode::UNAUTHORIZED {
        return Ok(r);
    }
    match fresh_preview_token(state, sb, used.as_deref()).await {
        Some(fresh) => with(Some(&fresh)).send().await,
        None => Ok(r),
    }
}

/// Ask agentd to stop (hold) or restart (release) one workspace on the user box.
async fn hold_workspace(
    state: &AppState,
    sb: &Sandbox,
    project_id: &str,
    hold: bool,
) -> Result<()> {
    let Some(url) = &sb.box_url else {
        return Err(anyhow!("user box has no url yet"));
    };
    let verb = if hold { "hold" } else { "release" };
    let r = send_to_box(state, sb, || {
        state
            .http
            .post(format!("{url}/w/{project_id}/{verb}"))
            .bearer_auth(&sb.mcp_token)
            .timeout(Duration::from_secs(60))
    })
    .await
    .with_context(|| format!("{verb} workspace"))?;
    if !r.status().is_success() {
        return Err(anyhow!("{verb} workspace: HTTP {}", r.status()));
    }
    Ok(())
}

/// Drop pi/Prime session files on a forked workspace so the new agent keeps files and memory
/// (the project's context) but does not resume the source agent's live chat session.
fn scrub_fork_sessions(dst: &std::path::Path) {
    for rel in [
        ".arbos/pi/sessions",
        ".arbos/prime/sessions",
        ".arbos/claude/sessions",
    ] {
        let p = dst.join(rel);
        let _ = std::fs::remove_dir_all(&p);
        let _ = std::fs::create_dir_all(&p);
    }
    let _ = std::fs::remove_file(dst.join(".arbos/.owner"));
}

/// Copy `/work/<source>` → `/work/<dest>` on the user's box when forking an agent.
///
/// The fork keeps the source project's folder and durable memory; chat UI history stays empty
/// (new pinned chat on the relay). Best effort: missing source dir is a no-op; failures are
/// logged. Waits briefly for the box to be reachable after a wake.
pub async fn copy_workspace_for_fork(
    state: &AppState,
    user_id: &str,
    source_project_id: &str,
    dest_project_id: &str,
) {
    if !safe_work_id(source_project_id) || !safe_work_id(dest_project_id) {
        tracing::warn!(%user_id, source_project_id, dest_project_id, "fork copy: bad project id");
        return;
    }
    if source_project_id == dest_project_id {
        return;
    }
    // Up to ~90s for a just-woken box to expose box_url / toolbox.
    let mut sb = None;
    for _ in 0..45 {
        match state.db.user_box(user_id) {
            Ok(Some(s)) if s.box_url.is_some() || s.provider != "daytona" => {
                sb = Some(s);
                break;
            }
            Ok(other) => sb = other,
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let Some(sb) = sb else {
        tracing::info!(%user_id, "fork copy: no box yet; nothing to copy");
        return;
    };
    match (
        Provider::parse(&sb.provider),
        &state.daytona,
        &sb.sandbox_id,
    ) {
        (Some(Provider::Daytona), Some(dt), Some(id)) => {
            // Hold both so agentd does not seed an empty dest while we copy, and so the
            // source SQLite is quiet enough for a consistent folder copy.
            let _ = hold_workspace(state, &sb, dest_project_id, true).await;
            let _ = hold_workspace(state, &sb, source_project_id, true).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let cmd = format!(
                "sudo -n sh -c 'if [ ! -d /work/{src} ]; then echo missing; exit 0; fi; \
                 for i in 1 2 3 4 5; do sudo -n rm -rf /work/{dst} && break; sleep 1; done; \
                 sudo -n cp -a /work/{src} /work/{dst} && \
                 sudo -n rm -rf /work/{dst}/.arbos/pi/sessions /work/{dst}/.arbos/prime/sessions \
                   /work/{dst}/.arbos/claude/sessions 2>/dev/null; \
                 sudo -n mkdir -p /work/{dst}/.arbos/pi/sessions; \
                 sudo -n rm -f /work/{dst}/.arbos/.owner; echo ok'",
                src = source_project_id,
                dst = dest_project_id
            );
            let result = dt.toolbox_exec(id, &cmd, 600).await;
            let _ = hold_workspace(state, &sb, source_project_id, false).await;
            let _ = hold_workspace(state, &sb, dest_project_id, false).await;
            match result {
                Ok(v) => {
                    let text = v
                        .get("result")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_owned();
                    if text.contains("missing") {
                        tracing::info!(%user_id, source_project_id, "fork copy: source workspace empty");
                    } else if v.get("exitCode").and_then(Value::as_i64).unwrap_or(1) != 0 {
                        tracing::warn!(%user_id, source_project_id, dest_project_id, %text, "fork copy failed");
                    } else {
                        tracing::info!(%user_id, source_project_id, dest_project_id, "fork copy done");
                    }
                }
                Err(e) => tracing::warn!(%user_id, error = %e, "fork copy"),
            }
        }
        (Some(Provider::Gvisor), _, Some(name)) => {
            let _ = hold_workspace(state, &sb, dest_project_id, true).await;
            let _ = hold_workspace(state, &sb, source_project_id, true).await;
            tokio::time::sleep(Duration::from_secs(1)).await;
            let cmd = format!(
                "if [ ! -d /work/{src} ]; then echo missing; exit 0; fi; \
                 rm -rf /work/{dst} && cp -a /work/{src} /work/{dst} && \
                 rm -rf /work/{dst}/.arbos/pi/sessions /work/{dst}/.arbos/prime/sessions /work/{dst}/.arbos/claude/sessions 2>/dev/null; \
                 mkdir -p /work/{dst}/.arbos/pi/sessions; rm -f /work/{dst}/.arbos/.owner; echo ok",
                src = source_project_id,
                dst = dest_project_id
            );
            let result = crate::fleet::exec_in_box(name, &cmd, Duration::from_secs(600)).await;
            let _ = hold_workspace(state, &sb, source_project_id, false).await;
            let _ = hold_workspace(state, &sb, dest_project_id, false).await;
            match result {
                Ok(text) if text.contains("missing") => {
                    tracing::info!(%user_id, source_project_id, "fork copy: source workspace empty")
                }
                Ok(_) => {
                    tracing::info!(%user_id, source_project_id, dest_project_id, "fork copy done")
                }
                Err(e) => tracing::warn!(%user_id, error = %e, "fork copy"),
            }
        }
        (Some(Provider::Process), _, _) | (Some(Provider::Docker), _, _) => {
            let base = state.cfg.data_dir.join("boxes").join(user_id).join("work");
            let src = base.join(source_project_id);
            let dst = base.join(dest_project_id);
            if !src.is_dir() {
                tracing::info!(%user_id, source_project_id, "fork copy: source workspace empty");
                return;
            }
            let _ = hold_workspace(state, &sb, dest_project_id, true).await;
            let _ = hold_workspace(state, &sb, source_project_id, true).await;
            let _ = std::fs::remove_dir_all(&dst);
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let st = tokio::process::Command::new("cp")
                .args(["-a", &src.display().to_string(), &dst.display().to_string()])
                .status()
                .await;
            if matches!(&st, Ok(s) if s.success()) {
                scrub_fork_sessions(&dst);
                tracing::info!(%user_id, source_project_id, dest_project_id, "fork copy done");
            } else {
                tracing::warn!(%user_id, source_project_id, dest_project_id, ?st, "fork copy failed");
            }
            let _ = hold_workspace(state, &sb, source_project_id, false).await;
            let _ = hold_workspace(state, &sb, dest_project_id, false).await;
        }
        _ => tracing::warn!(%user_id, provider = %sb.provider, "fork copy: unsupported provider"),
    }
}

/// Move a deleted project's `/work/<id>` to `/work/.archive/<id>` on the user's box (Daytona
/// through the toolbox shell, local providers on disk). Best effort: the purge removes whatever
/// is left seven days later.
pub async fn archive_workspace_dir(state: &AppState, user_id: &str, project_id: &str) {
    let Ok(Some(sb)) = state.db.user_box(user_id) else {
        return;
    };
    match (
        Provider::parse(&sb.provider),
        &state.daytona,
        &sb.sandbox_id,
    ) {
        (Some(Provider::Daytona), Some(dt), Some(id)) => {
            // Give the box's poll a moment to stop the workspace first.
            tokio::time::sleep(Duration::from_secs(20)).await;
            let cmd = format!(
                "sudo -n sh -c 'mkdir -p /work/.archive && if [ -d /work/{p} ]; then mv /work/{p} /work/.archive/{p}; fi; echo ok'",
                p = project_id
            );
            match dt.toolbox_exec(id, &cmd, 120).await {
                Ok(_) => {
                    tracing::info!(%user_id, project_id, "workspace folder archived on the box")
                }
                Err(e) => tracing::warn!(%user_id, project_id, error = %e, "workspace archive"),
            }
        }
        (Some(Provider::Gvisor), _, Some(name)) => {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let cmd = format!(
                "mkdir -p /work/.archive && if [ -d /work/{p} ]; then mv /work/{p} /work/.archive/{p}; fi; echo ok",
                p = project_id
            );
            match crate::fleet::exec_in_box(name, &cmd, Duration::from_secs(120)).await {
                Ok(_) => {
                    tracing::info!(%user_id, project_id, "workspace folder archived on the box")
                }
                Err(e) => tracing::warn!(%user_id, project_id, error = %e, "workspace archive"),
            }
        }
        (Some(Provider::Process), _, _) | (Some(Provider::Docker), _, _) => {
            let base = state.cfg.data_dir.join("boxes").join(user_id).join("work");
            let src = base.join(project_id);
            if src.exists() {
                let dst = base.join(".archive").join(project_id);
                let _ = std::fs::create_dir_all(dst.parent().unwrap_or(&base));
                let _ = std::fs::rename(&src, &dst);
            }
        }
        _ => {}
    }
}

/// Hourly: projects soft-deleted more than seven days ago lose their rows, rooms, blobs, and the
/// archived folder on the box.
pub async fn spawn_project_purge(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let cutoff = now_ms() - 7 * 24 * 3_600_000;
            for (project_id, user_id) in
                state.db.projects_deleted_before(cutoff).unwrap_or_default()
            {
                if let Ok(Some(sb)) = state.db.user_box(&user_id) {
                    if sb.provider == "gvisor" {
                        if let Some(name) = &sb.sandbox_id {
                            let _ = crate::fleet::exec_in_box(
                                name,
                                &format!("rm -rf /work/.archive/{project_id}; echo ok"),
                                Duration::from_secs(120),
                            )
                            .await;
                        }
                    } else if let (Some(dt), Some(id)) = (&state.daytona, &sb.sandbox_id) {
                        if sb.provider == "daytona" {
                            let _ = dt
                                .toolbox_exec(
                                    id,
                                    &format!("sudo -n rm -rf /work/.archive/{project_id}; echo ok"),
                                    120,
                                )
                                .await;
                        }
                    } else {
                        let _ = std::fs::remove_dir_all(
                            state
                                .cfg
                                .data_dir
                                .join("boxes")
                                .join(&user_id)
                                .join("work")
                                .join(".archive")
                                .join(&project_id),
                        );
                    }
                }
                let _ = std::fs::remove_dir_all(state.cfg.data_dir.join("blobs").join(&project_id));
                match state.db.purge_project(&project_id) {
                    Ok(rooms) => {
                        tracing::info!(project_id, rooms = rooms.len(), "deleted project purged")
                    }
                    Err(e) => tracing::warn!(project_id, error = %e, "project purge"),
                }
            }
        }
    });
}

/// Always-on (Jacob, 2026-10-15): every signed-in user's machine runs all the time. At boot and
/// every minute, any box that is not awake or waking — and that its user did not stop by hand —
/// is woken; a member without a box yet gets one. Manual Stop (`held_stopped`) and the kill
/// switch are the only things that keep a machine down.
pub async fn spawn_always_on(state: AppState) {
    if !state.cfg.always_on {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            let users = state.db.always_on_users().unwrap_or_default();
            for user_id in users {
                let (st, _) = state.box_state(&user_id).await;
                if matches!(st, BoxState::Awake | BoxState::Waking) {
                    continue;
                }
                if st == BoxState::Error {
                    // A stale-box error stays until the user asks again (see `explicit_wake`).
                    continue;
                }
                // Waitlisted (D89): no box until the admission tick frees a slot.
                if state.db.is_waitlisted(&user_id).unwrap_or(false) {
                    continue;
                }
                tracing::info!(%user_id, "always-on: waking");
                ensure_awake(&state, &user_id, "always-on").await;
            }
        }
    });
}

/// A sign of life this recent makes a box count as live for [`sandboxless_live_boxes`].
const LIVE_BOX_MS: i64 = 15 * 60_000;
/// A `box_creates` row this old, with no create running in this process, was cut short.
const CREATE_STALE_MS: i64 = 10 * 60_000;
const SANDBOXLESS_CHECK_EVERY: Duration = Duration::from_secs(600);
/// After boot the boxes need a moment to dial in again before "live" means anything.
const SANDBOXLESS_CHECK_FIRST: Duration = Duration::from_secs(120);

/// Box rows that name no sandbox while their box shows signs of life (#2262: a live box that
/// rolls and always-on cannot see), plus box creates that never finished. Read-only.
pub async fn sandboxless_live_boxes(state: &AppState) -> Vec<Value> {
    let now = now_ms();
    let recent = |t: Option<i64>| t.is_some_and(|t| now - t < LIVE_BOX_MS);
    let mut out = Vec::new();
    for b in state.db.list_boxes().unwrap_or_default() {
        if b.sandbox_id.is_some() {
            continue;
        }
        let reachable = box_reachable(state, &b.user_id).await;
        let meter = state.db.meter_tick(&b.user_id).ok().flatten();
        let meter_running = meter
            .as_ref()
            .is_some_and(|(at, st)| st == "running" && recent(Some(*at)));
        let hello_at = state
            .db
            .box_agentd(&b.user_id)
            .ok()
            .flatten()
            .map(|a| a.reported_at);
        if reachable || recent(b.last_seen_at) || meter_running || recent(hello_at) {
            out.push(json!({
                "kind": "sandboxless_live_row",
                "user_id": b.user_id, "provider": b.provider, "reachable": reachable,
                "last_seen_at": b.last_seen_at, "meter_tick": meter.map(|m| m.0), "hello_at": hello_at,
            }));
        }
    }
    let running: std::collections::HashSet<String> = state
        .restart
        .creating
        .lock()
        .expect("creating lock")
        .keys()
        .cloned()
        .collect();
    for c in state
        .db
        .unfinished_box_creates(now - CREATE_STALE_MS)
        .unwrap_or_default()
    {
        if !running.contains(&c.user_id) {
            out.push(json!({
                "kind": "unfinished_create",
                "user_id": c.user_id, "provider": c.provider, "sandbox_name": c.sandbox_name,
                "snapshot": c.snapshot, "replacing": c.replacing, "started_at": c.started_at,
            }));
        }
    }
    out
}

/// At boot (after the boxes had time to dial in) and every 10 minutes: log every finding of
/// [`sandboxless_live_boxes`]. Logs only; rows are re-attached by hand.
pub async fn spawn_sandboxless_box_check(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(SANDBOXLESS_CHECK_FIRST).await;
        loop {
            for f in sandboxless_live_boxes(&state).await {
                let user = f["user_id"].as_str().unwrap_or_default().to_owned();
                if f["kind"] == "unfinished_create" {
                    tracing::error!(user = %user, finding = %f, "box create never finished: its sandbox may run without a row naming it (#2262); not repaired");
                } else {
                    tracing::error!(user = %user, finding = %f, "box row has no sandbox but its box is live (#2262); not repaired");
                }
            }
            tokio::time::sleep(SANDBOXLESS_CHECK_EVERY).await;
        }
    });
}

/// Wake-job runner: 3 attempts with backoff, then S13 error (crdt-sync §8.4 step 5).
pub async fn spawn_wake_runner(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(750)).await;
            let jobs = match state.db.due_wake_jobs() {
                Ok(j) => j,
                Err(_) => continue,
            };
            for (user_id, reason, attempts) in jobs {
                let (st, _) = state.box_state(&user_id).await;
                if st == BoxState::Awake || st == BoxState::Error {
                    // Awake: the dial finished it. Error: the stale-box detector gave up; only an
                    // explicit wake/start re-arms it.
                    let _ = state.db.finish_wake_job(&user_id);
                    continue;
                }
                if state.upgrading.lock().await.contains_key(&user_id) {
                    // The upgrade starts the box it ends on (or the old one on rollback). A start
                    // from here would bounce a box mid-carry and write back a row read before
                    // the new box landed in it (#2262); the job waits for the upgrade.
                    continue;
                }
                // Mark the job as in flight by pushing its due time out; the box dial finishes it.
                let _ = state.db.retry_wake_job(&user_id, 120_000, "in flight");
                tracing::info!(%user_id, %reason, attempts, "waking box");
                match start_box(&state, &user_id).await {
                    Ok(()) => {}
                    Err(e) => {
                        tracing::warn!(%user_id, error = %e, "wake failed");
                        let n = state
                            .db
                            .retry_wake_job(&user_id, 5_000 * (attempts + 1), &e.to_string())
                            .unwrap_or(99);
                        if n >= 3 {
                            let _ = state.db.finish_wake_job(&user_id);
                            state
                                .set_box_state(&user_id, BoxState::Error, Some(&e.to_string()))
                                .await;
                        }
                    }
                }
            }
        }
    });
}

/// Keep-alive from the box while a turn runs (design §4.6): forward as "update last activity".
pub async fn keepalive(state: &AppState, user_id: &str, turn_running: bool) -> Result<()> {
    {
        // Eviction (D89) never takes a sandbox mid-turn.
        let mut t = state.turn_running.lock().await;
        if turn_running {
            t.insert(user_id.to_owned(), now_ms());
        } else {
            t.remove(user_id);
        }
    }
    let sb = state
        .db
        .user_box(user_id)?
        .ok_or_else(|| anyhow!("no box for user {user_id}"))?;
    if sb.provider == "daytona" {
        if let (Some(dt), Some(id)) = (&state.daytona, &sb.sandbox_id) {
            dt.refresh_activity(id).await?;
            let minutes = if state.cfg.always_on || turn_running {
                0
            } else {
                30
            };
            let _ = dt.set_autostop(id, minutes).await;
        }
    }
    state.db.touch_box(user_id)?;
    Ok(())
}

/// Current provider view of the user's box, for `GET /projects/{id}/status`.
pub async fn provider_status(state: &AppState, user_id: &str) -> Value {
    let sb = match state.db.user_box(user_id) {
        Ok(Some(s)) => s,
        _ => return json!({ "provider": null }),
    };
    let mut v = json!({
        "provider": sb.provider, "sandbox_id": sb.sandbox_id, "box_url": sb.box_url, "user_id": sb.user_id,
        "migrated_from": sb.migrated_from,
        "last_started_at": sb.last_started_at, "last_stopped_at": sb.last_stopped_at, "last_seen_at": sb.last_seen_at,
    });
    if sb.provider == "daytona" {
        if let (Some(dt), Some(id)) = (&state.daytona, &sb.sandbox_id) {
            if let Ok(s) = dt.get(id).await {
                v["daytona_state"] = json!(s.state);
                v["daytona_arbos_state"] = json!(s.state.arbos_state());
                v["auto_stop_interval"] = json!(s.auto_stop_interval);
                v["last_activity_at"] = json!(s.last_activity_at);
            }
        }
    }
    v
}

#[cfg(test)]
mod zero_project_tests {
    //! A box whose user has no project opens no room socket; its HTTP polls must still make it
    //! awake, keep the stale-box detector quiet, and put it to sleep only when they stop.

    use super::*;
    use crate::people::tests_support::person;
    use crate::state::test_support::test_state;
    use crate::state::AuthMode;

    #[tokio::test]
    async fn box_with_no_project_is_awake_through_its_http_heartbeat() {
        let state = test_state(AuthMode::Github);
        let (user, _dev) = person(&state, 1);
        assert!(state.db.projects_of_user(&user).unwrap().is_empty());

        // The wake started well past the hello deadline and no socket ever came.
        state.box_states.write().await.insert(
            user.clone(),
            (BoxState::Waking, now_ms() - HELLO_DEADLINE_MS - 5_000),
        );
        assert!(!box_reachable(&state, &user).await);

        // `GET /box/projects` from the box: awake, wake over, detector has nothing to do.
        box_seen_http(&state, &user).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);
        assert!(box_reachable(&state, &user).await);
        sweep_box_liveness(&state).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);
        assert!(
            !stale_wake_check(&state, &user, 0).await,
            "reachable boxes are never bounced"
        );

        // Had the relay already given up ("error"), the same poll brings it back.
        state
            .set_box_state(&user, BoxState::Error, Some("box cannot reach relay"))
            .await;
        box_seen_http(&state, &user).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);

        // A project created now finds an awake box: no wake job, state unchanged.
        let project_id = crate::people::tests_support::project_of(&state, &user, "First");
        ensure_awake_project(&state, &project_id, "api").await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Awake);
        assert!(state.wake_started.lock().await.get(&user).is_none());

        // The heartbeat stops (box died): after the grace period it is asleep, not "error".
        state
            .box_http_seen
            .lock()
            .await
            .insert(user.clone(), now_ms() - 120_000);
        sweep_box_liveness(&state).await;
        assert_eq!(state.box_state(&user).await.0, BoxState::Asleep);
    }
}
