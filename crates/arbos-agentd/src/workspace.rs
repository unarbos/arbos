//! One box, many workspaces (design D70). The `Supervisor` owns one `Workspace` per project of
//! the box's user: its own `Config` (dir, loopback ports), SQLite store, sync hub, `Agent` (pi
//! main session + subagents), hook/MCP/model-stub listeners, and command loop. Workspaces are
//! created from `/box/hello` and the `/box/projects` poll, and torn down when a project goes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::sync::{mpsc, RwLock, Semaphore};

use arbos_core::wire::PROJECT_DOC_CHAT_ID;

use crate::agent::Agent;
use crate::config::{BoxConfig, Config};
use crate::isolation::Isolation;
use crate::store::Store;
use crate::sync::{Incoming, SyncHub};
use crate::{hooks, http, mcp};

/// Subagents running at once across the whole box (4 per workspace, see `agent.rs`).
pub const BOX_MAX_SUBAGENTS: usize = 8;

pub struct Workspace {
    pub project_id: String,
    pub cfg: Arc<Config>,
    pub store: Store,
    pub sync: SyncHub,
    pub agent: Arc<Agent>,
    pub started_at: i64,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

pub struct Supervisor {
    pub cfg: BoxConfig,
    pub workspaces: RwLock<HashMap<String, Arc<Workspace>>>,
    /// Projects the relay asked us to keep stopped (migration copies files under them).
    pub held: RwLock<std::collections::HashSet<String>>,
    pub box_slots: Arc<Semaphore>,
    pub booted_at: i64,
    pub isolation: Isolation,
    /// Result of the cross-workspace read check (`None` until two workspaces exist).
    pub cross_read_blocked: RwLock<Option<bool>>,
    http: reqwest::Client,
    /// Next loopback port block; ports of removed workspaces are not reused within one boot.
    next_index: std::sync::atomic::AtomicU16,
}

impl Supervisor {
    pub fn new(cfg: BoxConfig) -> Arc<Self> {
        let isolation = Isolation::detect(cfg.local_box);
        Arc::new(Self {
            isolation,
            cross_read_blocked: RwLock::new(None),
            cfg,
            workspaces: RwLock::new(HashMap::new()),
            held: RwLock::new(std::collections::HashSet::new()),
            box_slots: Arc::new(Semaphore::new(BOX_MAX_SUBAGENTS)),
            booted_at: arbos_core::ids::now_ms(),
            http: reqwest::Client::new(),
            next_index: std::sync::atomic::AtomicU16::new(0),
        })
    }

    pub async fn get(&self, project_id: &str) -> Option<Arc<Workspace>> {
        self.workspaces.read().await.get(project_id).cloned()
    }

    pub async fn list(&self) -> Vec<Arc<Workspace>> {
        self.workspaces.read().await.values().cloned().collect()
    }

    /// The workspace whose store knows this chat (legacy `/history/{chat}` without a project).
    pub async fn find_chat(&self, chat_id: &str) -> Option<Arc<Workspace>> {
        for w in self.list().await {
            if w.store.chat(chat_id).ok().flatten().is_some() {
                return Some(w);
            }
        }
        None
    }

    pub async fn any_turn_running(&self) -> bool {
        self.list().await.iter().any(|w| w.agent.any_turn_running())
    }

    /// Box-level keep-alive: any workspace mid-turn keeps the box awake.
    pub async fn keepalive(&self, turn_running: bool) -> Result<()> {
        self.http
            .post(format!("{}/box/keepalive", self.cfg.relay_url))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({ "turn_running": turn_running }))
            .send()
            .await?;
        Ok(())
    }

    /// Create (or refresh) the workspace for one project as listed by the relay.
    pub async fn ensure_workspace(
        self: &Arc<Self>,
        project: &Value,
        chats: &[Value],
        pushes: &[Value],
    ) -> Result<Arc<Workspace>> {
        let project_id = project
            .get("id")
            .and_then(Value::as_str)
            .context("project without id")?
            .to_owned();
        if let Some(w) = self.get(&project_id).await {
            if let Some(owner) = project.get("user_id").and_then(Value::as_str) {
                w.agent.people.set_project_owner(owner);
            }
            // Settings and chats change without a restart.
            if let Err(e) = w.agent.settings_from_relay(project, chats).await {
                tracing::warn!(project = %project_id, error = %e, "settings from relay");
            }
            w.sync_chats(chats).await;
            let rev_moved = project
                .get("skills_rev")
                .and_then(Value::as_str)
                .is_some_and(|r| r != w.agent.skills.rev());
            if rev_moved || w.agent.skills.is_stale() {
                w.agent.refresh_skills().await;
            }
            return Ok(w);
        }
        let index = self
            .next_index
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let t0 = Instant::now();
        let mut cfg = self.cfg.workspace(project, index);
        tracing::info!(project = %project_id, dir = %cfg.work_dir.display(), mcp = cfg.mcp_port, "starting workspace");
        // Fork: copy the source agent's folder (files + memory) before first-boot seed.
        if let Some(src_id) = project.get("forked_from").and_then(Value::as_str) {
            if let Err(e) = clone_forked_workspace(&self.cfg.work_root, src_id, &cfg.work_dir) {
                tracing::warn!(project = %project_id, source = %src_id, error = %e, "fork clone");
            }
        }
        seed_state(&cfg)?;
        if self.isolation.enabled {
            let ra = self
                .isolation
                .prepare(&project_id, &cfg.work_dir, &cfg.state_dir)
                .with_context(|| format!("isolation for {project_id}"))?;
            tracing::info!(project = %project_id, user = %ra.user, uid = ra.uid, cgroup = ?ra.cgroup, "workspace identity");
            cfg.run_as = Some(ra);
        }
        let cfg = Arc::new(cfg);
        let store = Store::open(&cfg.state_dir.join("agentd").join("agentd.db"))?;
        let (tx, rx) = mpsc::channel::<Incoming>(1024);
        let sync = SyncHub::new(cfg.clone(), store.clone(), tx);
        let agent = Agent::new(
            cfg.clone(),
            store.clone(),
            sync.clone(),
            self.box_slots.clone(),
        );

        // Loopback servers first, so pi's extension can reach the hook API as soon as it loads.
        let hook_l = tokio::net::TcpListener::bind(("127.0.0.1", cfg.hook_port))
            .await
            .context("bind hook port")?;
        let mcp_l = tokio::net::TcpListener::bind(("127.0.0.1", cfg.mcp_port))
            .await
            .context("bind mcp port")?;
        let stub_l = tokio::net::TcpListener::bind(("127.0.0.1", cfg.proxy_port))
            .await
            .context("bind proxy stub port")?;
        let mut tasks = Vec::new();
        let (hook_app, mcp_app, stub_app) = (
            hooks::router(agent.clone()),
            mcp::router(agent.clone()),
            http::stub_router(agent.clone()),
        );
        tasks.push(tokio::spawn(async move {
            axum::serve(hook_l, hook_app).await.ok();
        }));
        tasks.push(tokio::spawn(async move {
            axum::serve(mcp_l, mcp_app).await.ok();
        }));
        tasks.push(tokio::spawn(async move {
            axum::serve(stub_l, stub_app).await.ok();
        }));

        for c in chats {
            if let (Some(id), Some(title)) = (
                c.get("id").and_then(Value::as_str),
                c.get("title").and_then(Value::as_str),
            ) {
                let _ = store.upsert_chat(
                    id,
                    title,
                    c.get("is_pinned").and_then(Value::as_bool).unwrap_or(false),
                    c.get("created_at").and_then(Value::as_i64).unwrap_or(0),
                );
            }
        }
        let known: Vec<String> = store.chats()?.into_iter().map(|c| c.id).collect();
        let initial_chat = known
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_owned());
        sync.open_room(PROJECT_DOC_CHAT_ID).await?;
        for id in &known {
            sync.open_room(id).await?;
        }
        // Skills index (F5) before the first turn; refreshed every 60 s, when `skills_rev` moves,
        // and on the `skills_changed` push.
        agent.refresh_skills().await;
        agent.start_pi(&initial_chat).await?;
        let now = arbos_core::ids::now_ms();
        sync.write_status(&[
            ("box", json!({ "state": "awake", "since": self.booted_at })),
            ("workspace", json!({ "state": "running", "since": now })),
            ("runningTurns", json!(0)),
            ("spendTodayUsd", json!(0.0)),
        ])
        .await;
        if let Some(owner) = project.get("user_id").and_then(Value::as_str) {
            agent.people.set_project_owner(owner);
        }
        if let Err(e) = agent.settings_from_relay(project, chats).await {
            tracing::warn!(project = %project_id, error = %e, "settings from relay");
        }
        agent.write_threads_index().await;
        let _ = agent
            .post_timing("workspace_start_ms", t0.elapsed().as_millis() as i64, None)
            .await;
        self.report_workspace_state(&project_id, "running").await;
        // Pushes queued while the box slept (routine runs, cross-project messages): dispatched once
        // the rooms are open and pi is up; later ones arrive through the push poller.
        for p in pushes {
            tracing::info!(project = %project_id, push = %p, "queued push delivered at boot");
            agent.dispatch_push(p.clone());
        }
        agent.spawn_push_poller();
        if let Err(e) = crate::vault::refresh_all(&agent).await {
            tracing::info!(project = %project_id, error = %e, "secrets: no grants yet");
        }
        tasks.push(agent.spawn_tree_builder());

        // Prime ask_user marker directory (D64 skill) → InputRequested.
        {
            let agent2 = agent.clone();
            let ask_dir = cfg.state_dir.join("ask");
            tasks.push(tokio::spawn(async move {
                let _ = std::fs::create_dir_all(&ask_dir);
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let Ok(rd) = std::fs::read_dir(&ask_dir) else {
                        continue;
                    };
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.extension().and_then(|x| x.to_str()) != Some("json") {
                            continue;
                        }
                        if let Some(v) = std::fs::read_to_string(&p)
                            .ok()
                            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        {
                            let q = v
                                .get("question")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned();
                            let sid = v
                                .get("session_id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned();
                            let widget = v.get("widget").cloned();
                            let _ = agent2.hook_ask_user(&q, widget.as_ref(), &sid).await;
                        }
                        let _ = std::fs::rename(&p, p.with_extension("done"));
                    }
                }
            }));
        }
        // Approval cards expire 24 h after they were posted (feature `approve`).
        {
            let agent2 = agent.clone();
            tasks.push(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    agent2.expire_approvals().await;
                }
            }));
        }
        // Command loop. Each command runs on its own task: `send`/`answer` serialize on the
        // agent's turn lock (FIFO), while `stop`/`steer` (Guide) and settings changes land at
        // once, mid-turn.
        {
            let agent2 = agent.clone();
            tasks.push(tokio::spawn(async move {
                let mut rx = rx;
                while let Some(inc) = rx.recv().await {
                    match inc {
                        Incoming::Command { chat_id, cmd } => {
                            let agent = agent2.clone();
                            tokio::spawn(async move {
                                let _ = agent.handle_command(&chat_id, cmd).await;
                            });
                        }
                        Incoming::CaughtUp { chat_id, head_seq } => {
                            tracing::info!(chat_id, head_seq, "room caught up");
                        }
                        Incoming::Status { chat_id, state } => {
                            tracing::debug!(chat_id, ?state, "relay status");
                        }
                        Incoming::ProjectDocChanged => {
                            let agent = agent2.clone();
                            tokio::spawn(async move {
                                agent.settings_from_doc().await;
                            });
                        }
                    }
                }
            }));
        }
        let ws = Arc::new(Workspace {
            project_id: project_id.clone(),
            cfg,
            store,
            sync,
            agent,
            started_at: now,
            tasks,
        });
        self.workspaces
            .write()
            .await
            .insert(project_id.clone(), ws.clone());
        tracing::info!(project = %project_id, ms = t0.elapsed().as_millis() as i64, "workspace running");
        self.check_isolation().await;
        Ok(ws)
    }

    /// Invariant (D70): with two or more workspaces, A's user cannot read B's folder. Re-run as
    /// workspaces come and go; the result shows in `/healthz`.
    pub async fn check_isolation(&self) {
        if !self.isolation.enabled {
            return;
        }
        let ws = self.list().await;
        let Some(a) = ws.first() else { return };
        let Some(b) = ws.get(1) else { return };
        let Some(ra) = &a.cfg.run_as else { return };
        match Isolation::cross_read_blocked(ra, &b.cfg.work_dir) {
            Ok(blocked) => {
                if !blocked {
                    tracing::error!(a = %a.project_id, b = %b.project_id, "ISOLATION BREACH: workspace A can read workspace B");
                }
                *self.cross_read_blocked.write().await = Some(blocked);
            }
            Err(e) => tracing::warn!(error = %e, "isolation check failed to run"),
        }
    }

    /// Tell the relay (and the project doc) a workspace's state.
    pub async fn report_workspace_state(&self, project_id: &str, state: &str) {
        let _ = self
            .http
            .post(format!(
                "{}/projects/{}/workspace",
                self.cfg.relay_url, project_id
            ))
            .bearer_auth(&self.cfg.box_token)
            .json(&json!({ "state": state }))
            .send()
            .await;
    }

    /// A project disappeared from the relay: stop its children, keep its files.
    pub async fn remove_workspace(&self, project_id: &str) {
        let Some(w) = self.workspaces.write().await.remove(project_id) else {
            return;
        };
        tracing::info!(project = %project_id, "stopping workspace");
        w.sync
            .write_status(&[(
                "workspace",
                json!({ "state": "stopped", "since": arbos_core::ids::now_ms() }),
            )])
            .await;
        let _ = tokio::time::timeout(Duration::from_secs(20), w.agent.shutdown()).await;
        w.sync.persist_all().await;
        for t in &w.tasks {
            t.abort();
        }
        self.report_workspace_state(project_id, "stopped").await;
    }

    /// Apply one `/box/hello` or `/box/projects` answer: start new workspaces, refresh the rest,
    /// drop the ones the relay no longer lists.
    pub async fn apply_listing(self: &Arc<Self>, listing: &Value) {
        let entries = listing
            .get("workspaces")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut seen = Vec::new();
        let held = self.held.read().await.clone();
        for e in &entries {
            let Some(project) = e.get("project") else {
                continue;
            };
            if let Some(id) = project.get("id").and_then(Value::as_str) {
                if held.contains(id) {
                    seen.push(id.to_owned());
                    continue;
                }
            }
            let chats = e
                .get("chats")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let pushes = e
                .get("pushes")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let people_chats = e
                .get("people_chats")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            match self.ensure_workspace(project, &chats, &pushes).await {
                Ok(w) => {
                    w.agent.note_people_chats(&people_chats, true).await;
                    seen.push(w.project_id.clone());
                }
                Err(err) => {
                    tracing::error!(error = %err, project = ?project.get("id"), "workspace failed to start");
                    if let Some(id) = project.get("id").and_then(Value::as_str) {
                        self.report_workspace_state(id, "error").await;
                    }
                }
            }
        }
        let gone: Vec<String> = self
            .workspaces
            .read()
            .await
            .keys()
            .filter(|k| !seen.contains(k))
            .cloned()
            .collect();
        for id in gone {
            self.remove_workspace(&id).await;
        }
    }

    /// Migration support: stop a workspace and keep it stopped while the relay replaces its files.
    pub async fn hold(&self, project_id: &str) {
        self.held.write().await.insert(project_id.to_owned());
        self.remove_workspace(project_id).await;
    }

    /// The files are in place: the next poll starts the workspace again from them.
    pub async fn release(&self, project_id: &str) {
        self.held.write().await.remove(project_id);
    }

    /// SIGTERM: every workspace persists and stops; the doc records the box as asleep.
    pub async fn shutdown(&self) {
        let all = self.list().await;
        let now = arbos_core::ids::now_ms();
        for w in &all {
            w.sync
                .write_status(&[
                    ("box", json!({ "state": "asleep", "since": now })),
                    ("workspace", json!({ "state": "stopped", "since": now })),
                ])
                .await;
        }
        // Every workspace at once (10 workspaces × a 5 s child grace would otherwise outlive the
        // caller's budget and leave orphaned children behind).
        let mut joins = Vec::new();
        for w in &all {
            let w = w.clone();
            joins.push(tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(25), w.agent.shutdown()).await;
                w.sync.persist_all().await;
            }));
        }
        for j in joins {
            let _ = j.await;
        }
    }
}

impl Workspace {
    /// New chats from the registry get a room; titles land in the local index.
    pub async fn sync_chats(&self, chats: &[Value]) {
        for c in chats {
            if let Some(id) = c.get("id").and_then(Value::as_str) {
                let _ = self.store.upsert_chat(
                    id,
                    c.get("title").and_then(Value::as_str).unwrap_or("New chat"),
                    c.get("is_pinned").and_then(Value::as_bool).unwrap_or(false),
                    c.get("created_at").and_then(Value::as_i64).unwrap_or(0),
                );
                if self.sync.host(id).await.is_none() {
                    if let Err(e) = self.sync.open_room(id).await {
                        tracing::warn!(error = %e, id, "open room");
                    }
                }
            }
        }
    }
}

/// Copy `/work/<source>` into a brand-new fork workspace. Skips when the dest already exists
/// (already seeded or cloned). Clears pi/Prime sessions so the new agent keeps files and
/// memory but does not resume the source chat session.
fn clone_forked_workspace(
    work_root: &std::path::Path,
    source_id: &str,
    dest: &std::path::Path,
) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    if source_id.is_empty()
        || source_id.len() > 64
        || !source_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(anyhow::anyhow!("bad forked_from id"));
    }
    let src = work_root.join(source_id);
    if !src.is_dir() {
        tracing::info!(source = %source_id, "fork clone: source workspace missing");
        return Ok(());
    }
    // Refuse to leave the work root (symlink / `..` games).
    let src_canon = src.canonicalize().unwrap_or(src.clone());
    let root_canon = work_root
        .canonicalize()
        .unwrap_or_else(|_| work_root.to_path_buf());
    if !src_canon.starts_with(&root_canon) {
        return Err(anyhow::anyhow!("fork source outside work root"));
    }
    tracing::info!(source = %source_id, dest = %dest.display(), "fork clone: copying workspace");
    let st = std::process::Command::new("cp")
        .args([
            "-a",
            &src.display().to_string(),
            &dest.display().to_string(),
        ])
        .status()
        .context("cp -a for fork")?;
    if !st.success() {
        return Err(anyhow::anyhow!("cp -a failed for fork"));
    }
    for rel in [
        ".arbos/pi/sessions",
        ".arbos/prime/sessions",
        ".arbos/claude/sessions",
    ] {
        let p = dest.join(rel);
        let _ = std::fs::remove_dir_all(&p);
        let _ = std::fs::create_dir_all(&p);
    }
    let _ = std::fs::remove_file(dest.join(".arbos/.owner"));
    Ok(())
}

/// First-boot seeding of one workspace (sandbox-image.md §4 step 5): pi seed → state dir,
/// AGENTS.md, mcp.json, the model stub as pi/Prime provider.
pub fn seed_state(cfg: &Config) -> Result<()> {
    std::fs::create_dir_all(&cfg.state_dir)?;
    std::fs::create_dir_all(cfg.pi_dir())?;
    std::fs::create_dir_all(cfg.pi_sessions_dir())?;
    std::fs::create_dir_all(cfg.blobs_dir())?;
    std::fs::create_dir_all(cfg.state_dir.join("logs"))?;
    std::fs::create_dir_all(cfg.prime_dir())?;
    std::fs::create_dir_all(&cfg.work_dir)?;
    std::fs::create_dir_all(crate::media::out_dir(cfg))?;
    let seed = std::path::Path::new("/etc/arbos/pi-seed");
    if seed.exists() {
        copy_dir(seed, &cfg.pi_dir())?;
    }
    let claude_seed = std::path::Path::new("/etc/arbos/claude-seed/settings.json");
    let claude_dir = cfg.state_dir.join("claude");
    if claude_seed.exists() && !claude_dir.join("settings.json").exists() {
        std::fs::create_dir_all(&claude_dir)?;
        std::fs::copy(claude_seed, claude_dir.join("settings.json"))?;
    }
    // models.json: the relay proxy as provider `arbos-relay` through the loopback stub (D2).
    // The built-in `openrouter` provider pointed at the loopback stub keeps pi's OpenRouter behaviour
    // (cache_control breakpoints for Anthropic models, usage accounting) while the key stays on the relay.
    // `ARBOS_CONTEXT_WINDOW_TOKENS_OVERRIDE`: when set, forces every model stub's contextWindow (unset = 1_000_000).
    let context_window = context_window_tokens();
    let models = json!({
        "providers": {
            "openrouter": { "baseUrl": format!("{}/v1", cfg.proxy_base()), "apiKey": cfg.box_token },
            "arbos-relay": {
                "baseUrl": format!("{}/v1", cfg.proxy_base()),
                "api": "openai-completions",
                "apiKey": cfg.box_token,
                "models": [
                    { "id": cfg.talking_model, "name": format!("{} (relay)", cfg.talking_model), "reasoning": false, "input": ["text", "image"], "contextWindow": context_window, "maxTokens": 32000, "cost": { "input": 0.1, "output": 0.5, "cacheRead": 0.01, "cacheWrite": 0.125 } },
                    { "id": cfg.subagent_model, "name": format!("{} (relay)", cfg.subagent_model), "reasoning": true, "input": ["text", "image"], "contextWindow": context_window, "maxTokens": 64000, "cost": { "input": 2, "output": 10, "cacheRead": 0.2, "cacheWrite": 2.5 } }
                ]
            }
        }
    });
    std::fs::write(
        cfg.pi_dir().join("models.json"),
        serde_json::to_string_pretty(&models)?,
    )?;
    std::fs::write(
        cfg.prime_dir().join("models.json"),
        serde_json::to_string_pretty(&models)?,
    )?;
    let settings = json!({
        "defaultProvider": cfg.pi_provider(), "defaultModel": cfg.talking_model, "defaultThinkingLevel": "off",
        "defaultTools": ["read", "bash", "edit", "write", "codemode"],
        "compaction": { "enabled": true, "reserveTokens": 16384, "keepRecentTokens": 20000 },
        "hideThinkingBlock": true
    });
    std::fs::write(
        cfg.pi_dir().join("settings.json"),
        serde_json::to_string_pretty(&settings)?,
    )?;
    // Project tools reach the main agent through codemode only: 14 direct schemas cost ~4k prefix tokens (D68).
    // The bearer is expanded by pi from the per-role env (main vs subagent token, D69).
    let mcp = json!({ "mcpServers": { "arbos": { "url": cfg.mcp_url(), "headers": { "Authorization": "Bearer ${ARBOS_MCP_TOKEN}" }, "exposure": "codemode" } } });
    std::fs::write(
        cfg.pi_dir().join("mcp.json"),
        serde_json::to_string_pretty(&mcp)?,
    )?;
    let prime_settings_path = cfg.prime_dir().join("settings.json");
    let mut prime_settings: Value = std::fs::read_to_string(&prime_settings_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({}));
    // Same shape `prime-agent mcp add arbos --url … --bearer-token-env-var ARBOS_MCP_TOKEN` writes.
    prime_settings["mcpServers"] = json!({ "arbos": { "type": "http", "url": cfg.mcp_url(), "bearer_token_env_var": "ARBOS_MCP_TOKEN", "bearerTokenEnvVar": "ARBOS_MCP_TOKEN" } });
    std::fs::write(
        &prime_settings_path,
        serde_json::to_string_pretty(&prime_settings)?,
    )?;
    let agents_md = cfg.work_dir.join("AGENTS.md");
    if !agents_md.exists() {
        let src = std::path::Path::new("/etc/arbos/AGENTS.md");
        if src.exists() {
            std::fs::copy(src, &agents_md)?;
        } else {
            std::fs::write(&agents_md, include_str!("../../../image/AGENTS.md"))?;
        }
    }
    if !cfg.work_dir.join(".git").exists() {
        let _ = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&cfg.work_dir)
            .status();
    }
    Ok(())
}

/// Provider context-window size for model stubs. `ARBOS_CONTEXT_WINDOW_TOKENS_OVERRIDE` (unset = 1_000_000).
fn context_window_tokens() -> u64 {
    std::env::var("ARBOS_CONTEXT_WINDOW_TOKENS_OVERRIDE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1_000_000)
}

fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let target = dst.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &target)?;
        } else if !target.exists() {
            std::fs::copy(&p, &target)?;
        }
    }
    Ok(())
}
