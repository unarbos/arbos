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

use arbos_core::doc::{encode_vv, ChatDoc, CommandStatus, MessageStatus};
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

        let mut forks: Vec<(String, String)> = Vec::new();
        for c in chats {
            if let (Some(id), Some(title)) = (
                c.get("id").and_then(Value::as_str),
                c.get("title").and_then(Value::as_str),
            ) {
                let first_sight = store.chat(id).ok().flatten().is_none();
                let _ = store.upsert_chat(
                    id,
                    title,
                    c.get("is_pinned").and_then(Value::as_bool).unwrap_or(false),
                    c.get("created_at").and_then(Value::as_i64).unwrap_or(0),
                );
                if first_sight {
                    if let Some(src) = c.get("forked_from").and_then(Value::as_str) {
                        forks.push((id.to_owned(), src.to_owned()));
                    }
                }
            }
        }
        // Forked chats (#1257) seen for the first time at boot: seed before their room opens.
        for (id, src) in forks {
            if let Err(e) = seed_forked_chat(&cfg, &store, &sync, &project_id, &id, &src).await {
                tracing::warn!(error = %e, id, source = src, "fork chat seed");
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
        // After a restart nothing is mid-flight: clear leftover `running` /
        // `queued` rows and close their streaming progress messages so the
        // working indicator and working-set text cannot stick forever.
        agent.reclaim_orphan_subagents(true).await;
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
        agent.spawn_goals_poller();
        if let Err(e) = crate::vault::refresh_all(&agent).await {
            tracing::info!(project = %project_id, error = %e, "secrets: no grants yet");
        }
        if let Err(e) = agent.refresh_shell_grants().await {
            tracing::info!(project = %project_id, error = %e, "shell grants: none yet");
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
        w.sync.close_all().await;
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
                    // The owner's profile (address name) rides on every listing: the agent
                    // addresses the owner from it, live, never from a row's `senderName`.
                    if let Some(o) = e.get("owner").or_else(|| listing.get("owner")) {
                        let before = w.agent.people.owner_profile();
                        w.agent.people.set_owner_profile(o);
                        if w.agent.people.owner_profile() != before {
                            // The prompt's "Your owner" section follows the profile; the pi
                            // session re-reads it (at once when idle, else after the turn).
                            w.agent
                                .refresh_session_prompt("owner profile changed")
                                .await;
                        }
                        let names: Vec<String> = ["address_name", "name"]
                            .iter()
                            .filter_map(|k| o.get(*k).and_then(Value::as_str))
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .chain(
                                o.get("former_names")
                                    .and_then(Value::as_array)
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_str)
                                    .map(str::to_owned),
                            )
                            .collect();
                        w.agent.memory.set_owner_names(names);
                    }
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
    /// A chat forked from another (#1257) is seeded from its source first.
    pub async fn sync_chats(&self, chats: &[Value]) {
        for c in chats {
            if let Some(id) = c.get("id").and_then(Value::as_str) {
                let first_sight = self.store.chat(id).ok().flatten().is_none();
                let _ = self.store.upsert_chat(
                    id,
                    c.get("title").and_then(Value::as_str).unwrap_or("New chat"),
                    c.get("is_pinned").and_then(Value::as_bool).unwrap_or(false),
                    c.get("created_at").and_then(Value::as_i64).unwrap_or(0),
                );
                if self.sync.host(id).await.is_none() {
                    if first_sight {
                        if let Some(src) = c.get("forked_from").and_then(Value::as_str) {
                            if let Err(e) = seed_forked_chat(
                                &self.cfg,
                                &self.store,
                                &self.sync,
                                &self.project_id,
                                id,
                                src,
                            )
                            .await
                            {
                                tracing::warn!(error = %e, id, source = src, "fork chat seed");
                            }
                        }
                    }
                    if let Err(e) = self.sync.open_room(id).await {
                        tracing::warn!(error = %e, id, "open room");
                    }
                }
            }
        }
    }
}

/// Session fork (#1257): before the new chat's room opens, copy the source
/// chat's transcript into it (meta rewritten to the new id, commands marked
/// handled so nothing re-runs, a reply still streaming closed as cancelled)
/// and give the new chat the source's pi session file (the model's context)
/// and compaction summary. The room is saved with an empty acked frontier,
/// so its first sync pushes the whole transcript to the relay and every
/// device sees the history at once.
async fn seed_forked_chat(
    cfg: &Config,
    store: &Store,
    sync: &SyncHub,
    project_id: &str,
    chat_id: &str,
    source_id: &str,
) -> Result<()> {
    if chat_id == source_id {
        return Ok(());
    }
    let source_room = arbos_core::wire::room_name(project_id, source_id);
    let snapshot = match sync.host(source_id).await {
        Some(h) => Some(h.lock().await.doc.snapshot()),
        None => store
            .room_doc(&source_room)?
            .and_then(|(_, snap, _)| snap)
            .filter(|s| !s.is_empty()),
    };
    if let Some(snap) = snapshot {
        let forked = fork_doc_snapshot(&snap, sync.peer_id, chat_id, project_id)?;
        let room = arbos_core::wire::room_name(project_id, chat_id);
        let empty = encode_vv(&loro::VersionVector::default());
        store.save_room_doc(&room, 0, &forked, &empty)?;
    }
    if let Some(src) = store.chat(source_id)? {
        if let Some(path) = src.pi_session_path.as_deref().map(std::path::Path::new) {
            if path.exists() {
                match copy_pi_session(cfg, path, chat_id) {
                    Ok(new_path) => {
                        store.set_chat_session_path(chat_id, &new_path.display().to_string())?
                    }
                    Err(e) => tracing::warn!(error = %e, chat_id, "fork: pi session copy"),
                }
            }
        }
        if let (Some(summary), Some(through)) = (src.compaction_summary, src.compacted_through) {
            store.set_chat_compaction(chat_id, &summary, &through)?;
        }
    }
    tracing::info!(chat_id, source = source_id, "fork chat seeded");
    Ok(())
}

/// The forked chat's doc as a snapshot: the source transcript with `meta`
/// rewritten to the new chat, every command marked done (nothing re-runs in
/// the fork) and a reply still streaming closed as cancelled.
fn fork_doc_snapshot(
    source_snapshot: &[u8],
    peer: u64,
    chat_id: &str,
    project_id: &str,
) -> Result<Vec<u8>> {
    let doc = ChatDoc::from_snapshot(Some(peer), true, source_snapshot)?;
    doc.init_meta(chat_id, project_id, 1);
    for cmd in doc.commands() {
        if cmd.status != CommandStatus::Done.as_str() {
            doc.set_command_status(&cmd.id, CommandStatus::Done, None);
        }
    }
    let streaming: Vec<String> = doc
        .messages()
        .into_iter()
        .filter(|m| m.status == "streaming")
        .map(|m| m.id)
        .collect();
    for id in streaming {
        doc.finish_message(&id, MessageStatus::Cancelled);
    }
    Ok(doc.snapshot())
}

/// A pi session file's text for a forked chat: same entries, the header's
/// `id` rewritten to the new chat so pi treats it as that chat's session.
fn fork_session_text(text: &str, chat_id: &str) -> String {
    let mut lines = text.lines();
    let mut out = String::with_capacity(text.len());
    if let Some(first) = lines.next() {
        let header = match serde_json::from_str::<Value>(first) {
            Ok(mut v) if v.get("type").and_then(Value::as_str) == Some("session") => {
                v["id"] = json!(chat_id);
                v["timestamp"] = json!(chrono::Utc::now().to_rfc3339());
                v.to_string()
            }
            _ => first.to_owned(),
        };
        out.push_str(&header);
        out.push('\n');
    }
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Copy a pi session file for a forked chat into the sessions dir.
fn copy_pi_session(
    cfg: &Config,
    source: &std::path::Path,
    chat_id: &str,
) -> Result<std::path::PathBuf> {
    let dir = cfg.pi_sessions_dir();
    std::fs::create_dir_all(&dir)?;
    let text = std::fs::read_to_string(source).context("read source session")?;
    let out = fork_session_text(&text, chat_id);
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ");
    let path = dir.join(format!("{ts}_{chat_id}.jsonl"));
    std::fs::write(&path, out).context("write forked session")?;
    if let Some(ra) = &cfg.run_as {
        let _ = std::os::unix::fs::chown(&path, Some(ra.uid), Some(ra.gid));
    }
    Ok(path)
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
    write_models_json(cfg, &cfg.talking_model, &cfg.subagent_model)?;
    let settings = json!({
        "defaultProvider": cfg.pi_provider(), "defaultModel": cfg.talking_model, "defaultThinkingLevel": "off",
        "defaultTools": ["read", "bash", "edit", "write", "codemode"],
        "compaction": { "enabled": true, "reserveTokens": COMPACTION_RESERVE_TOKENS, "keepRecentTokens": KEEP_RECENT_TOKENS },
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

/// The context window pi plans against (and compacts under). `ARBOS_CONTEXT_WINDOW_TOKENS_OVERRIDE`
/// (unset = [`DEFAULT_CONTEXT_WINDOW_TOKENS`]).
pub fn context_window_tokens() -> u64 {
    std::env::var("ARBOS_CONTEXT_WINDOW_TOKENS_OVERRIDE")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS)
}

/// models.json for pi (and Prime): the relay proxy as provider `arbos-relay` through the loopback
/// stub (D2). The built-in `openrouter` provider pointed at the loopback stub keeps pi's OpenRouter
/// behaviour (cache_control breakpoints for Anthropic models, usage accounting) while the key stays
/// on the relay. Every model stub carries [`context_window_tokens`] as its window, and the live
/// talking / subagent models of the built-in provider get the same window through `modelOverrides`
/// — pi's own registry says 1M for them, so a chat's session never compacted: Lium's group session
/// reached 242k tokens, every turn rewrote a $2.6–3 cache and the provider refused all of them
/// (feedback lium-empty-reply-2026-10-21). Rewritten when the models change (`apply_models`); pi
/// reads it at (re)spawn.
pub fn write_models_json(cfg: &Config, talking: &str, subagent: &str) -> Result<()> {
    let context_window = context_window_tokens();
    let mut window_overrides = serde_json::Map::new();
    // The research-job worker model (cost knob 1) must be in pi's catalog too; it is usually the
    // talking model already, but a user may talk on another one.
    let research = cfg.research_worker_model();
    let mut ids = vec![talking, subagent];
    if let Some(r) = research.as_deref() {
        if !ids.contains(&r) {
            ids.push(r);
        }
    }
    for id in &ids {
        window_overrides.insert((*id).to_owned(), json!({ "contextWindow": context_window }));
    }
    let mut relay_models = vec![
        json!({ "id": talking, "name": format!("{talking} (relay)"), "reasoning": false, "input": ["text", "image"], "contextWindow": context_window, "maxTokens": 32000, "cost": { "input": 0.1, "output": 0.5, "cacheRead": 0.01, "cacheWrite": 0.125 } }),
        json!({ "id": subagent, "name": format!("{subagent} (relay)"), "reasoning": true, "input": ["text", "image"], "contextWindow": context_window, "maxTokens": 64000, "cost": { "input": 2, "output": 10, "cacheRead": 0.2, "cacheWrite": 2.5 } }),
    ];
    if let Some(r) = research
        .as_deref()
        .filter(|r| *r != talking && *r != subagent)
    {
        relay_models.push(json!({ "id": r, "name": format!("{r} (relay)"), "reasoning": false, "input": ["text", "image"], "contextWindow": context_window, "maxTokens": 32000, "cost": { "input": 0.1, "output": 0.5, "cacheRead": 0.01, "cacheWrite": 0.125 } }));
    }
    let mut models = json!({
        "providers": {
            "openrouter": { "baseUrl": format!("{}/v1", cfg.proxy_base()), "apiKey": cfg.box_token },
            "arbos-relay": {
                "baseUrl": format!("{}/v1", cfg.proxy_base()),
                "api": "openai-completions",
                "apiKey": cfg.box_token,
                "models": relay_models
            }
        }
    });
    // Prime reads the same file but not pi's `modelOverrides` key: write its copy first.
    std::fs::write(
        cfg.prime_dir().join("models.json"),
        serde_json::to_string_pretty(&models)?,
    )?;
    models["providers"]["openrouter"]["modelOverrides"] = Value::Object(window_overrides);
    std::fs::write(
        cfg.pi_dir().join("models.json"),
        serde_json::to_string_pretty(&models)?,
    )?;
    Ok(())
}

/// The hard ceiling of a chat's model window (D91). pi plans against it and compacts on its own
/// at `window − reserve` (≈ 112k), and a provider overflow at this size is answered with a
/// compact-and-retry — so the window never passes it. Was 200k (#1229, Anthropic's standard
/// window); 128k halves the worst-case cache write and keeps every call under the 5× Haiku tier.
/// The usual working window is far smaller: agentd compacts at [`DEFAULT_COMPACT_AT_TOKENS`].
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 128_000;

/// Soft compaction (D91): after a turn that leaves the chat's session above this many tokens,
/// agentd asks pi to compact — `keepRecentTokens` (20k) of recent turns stay verbatim and the rest
/// becomes one rolling summary, so the window settles at ≈ 20–30k. Older detail is not lost: it
/// stays in the memory tree and the walk brings it back when a question needs it.
/// `ARBOS_CONTEXT_COMPACT_AT_TOKENS` overrides; `0` leaves compaction to pi's hard threshold only.
pub const DEFAULT_COMPACT_AT_TOKENS: u64 = 64_000;

/// Tokens pi keeps verbatim when it compacts (`compaction.keepRecentTokens` in its settings).
pub const KEEP_RECENT_TOKENS: u64 = 20_000;

/// pi's `compaction.reserveTokens`: it caps the rolling summary at 0.8× this (6.5k) and sets
/// pi's own compaction threshold at `window − reserve` (120k, under the 128k ceiling). The
/// default 16k let the summary run to 13k, which with the 20k kept tail and one big turn put the
/// window back over the soft threshold right after a compaction.
pub const COMPACTION_RESERVE_TOKENS: u64 = 8_192;

/// After a soft compaction the next one waits until the window has grown by at least this much
/// past where it landed (the window seen on the first turn after the compaction): a turn large
/// enough to land back above the threshold on its own (an 8k paste, a big retrieved block) must
/// not rewrite the cache on every turn. The 128k ceiling still catches a chat that keeps growing.
pub const COMPACTION_HYSTERESIS_TOKENS: u64 = 16_000;

pub fn compact_at_tokens() -> u64 {
    match std::env::var("ARBOS_CONTEXT_COMPACT_AT_TOKENS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
    {
        Some(n) => n,
        None => DEFAULT_COMPACT_AT_TOKENS,
    }
}

/// Seed files are the image's: a file is copied when missing **or when its bytes differ** from the
/// seed, so an image roll updates `arbos-pi-ext` (and every other seed file) in a workspace that
/// already exists. Before this, an existing workspace kept the extension of the image it was first
/// created on — a copy-if-missing rule — and never saw later deny rules, hooks or guardrails.
///
/// A symlink in the seed stays a symlink in the workspace. `pi-seed/extensions/arbos-pi-ext/
/// node_modules/{typebox,@earendil-works}` point at pi's own package tree; following them copied
/// 18 000 files (119 MB) into every project and emptied the 131 072-inode work image of the
/// lt-eval box after seven projects (2026-10-26, "disk full" at 87 %). A copy an earlier image
/// made in place of a seed symlink is removed and replaced by the link.
fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let target = dst.join(e.file_name());
        let meta = std::fs::symlink_metadata(&p)?;
        if meta.file_type().is_symlink() {
            seed_symlink(&p, &target)?;
        } else if meta.is_dir() {
            copy_dir(&p, &target)?;
        } else if seed_file_differs(&p, &target) {
            std::fs::copy(&p, &target)?;
        }
    }
    Ok(())
}

/// Recreate seed symlink `seed` at `target`: nothing to do when the same link is there already;
/// a file or a (dereferenced) directory tree in its place is removed first.
fn seed_symlink(seed: &std::path::Path, target: &std::path::Path) -> Result<()> {
    let link = std::fs::read_link(seed)?;
    match std::fs::symlink_metadata(target) {
        Ok(m) if m.file_type().is_symlink() => {
            if std::fs::read_link(target).ok().as_deref() == Some(link.as_path()) {
                return Ok(());
            }
            std::fs::remove_file(target)?;
        }
        Ok(m) if m.is_dir() => {
            tracing::warn!(target = %target.display(), "seed symlink was copied as a tree; replacing it with the link");
            std::fs::remove_dir_all(target)?;
        }
        Ok(_) => std::fs::remove_file(target)?,
        Err(_) => {}
    }
    std::os::unix::fs::symlink(&link, target)?;
    Ok(())
}

/// `true` when `target` is missing or does not hold the bytes of `seed` (sizes first, then contents).
fn seed_file_differs(seed: &std::path::Path, target: &std::path::Path) -> bool {
    let (Ok(sm), Ok(tm)) = (std::fs::metadata(seed), std::fs::metadata(target)) else {
        return true;
    };
    if sm.len() != tm.len() {
        return true;
    }
    match (std::fs::read(seed), std::fs::read(target)) {
        (Ok(a), Ok(b)) => a != b,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    /// D91: the soft threshold reads its override and defaults to the design's 64k.
    #[test]
    fn compact_at_tokens_default_and_override() {
        std::env::remove_var("ARBOS_CONTEXT_COMPACT_AT_TOKENS");
        assert_eq!(super::compact_at_tokens(), super::DEFAULT_COMPACT_AT_TOKENS);
        std::env::set_var("ARBOS_CONTEXT_COMPACT_AT_TOKENS", "0");
        assert_eq!(super::compact_at_tokens(), 0);
        std::env::set_var("ARBOS_CONTEXT_COMPACT_AT_TOKENS", "90000");
        assert_eq!(super::compact_at_tokens(), 90_000);
        std::env::remove_var("ARBOS_CONTEXT_COMPACT_AT_TOKENS");
    }
    use super::*;

    /// 2026-10-26: a seed symlink must stay a symlink in the workspace, and a tree an older
    /// image copied in its place is replaced by the link (that tree cost 18 000 inodes per project).
    #[test]
    fn seed_symlinks_are_linked_not_copied() {
        let root = std::env::temp_dir().join(format!("arbos-seed-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let seed = root.join("seed");
        let shared = root.join("shared").join("pkg");
        std::fs::create_dir_all(seed.join("node_modules")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("a.js"), "x").unwrap();
        std::fs::write(seed.join("index.ts"), "ext").unwrap();
        std::os::unix::fs::symlink(&shared, seed.join("node_modules").join("pkg")).unwrap();
        // The workspace already holds a dereferenced copy from an earlier image.
        let ws = root.join("ws");
        std::fs::create_dir_all(ws.join("node_modules").join("pkg").join("deep")).unwrap();
        std::fs::write(
            ws.join("node_modules")
                .join("pkg")
                .join("deep")
                .join("b.js"),
            "y",
        )
        .unwrap();
        copy_dir(&seed, &ws).unwrap();
        let linked = ws.join("node_modules").join("pkg");
        assert!(std::fs::symlink_metadata(&linked)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&linked).unwrap(), shared);
        assert_eq!(std::fs::read_to_string(ws.join("index.ts")).unwrap(), "ext");
        // Idempotent.
        copy_dir(&seed, &ws).unwrap();
        assert!(std::fs::symlink_metadata(&linked)
            .unwrap()
            .file_type()
            .is_symlink());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #1851 regression bound: a fresh project's own bytes from the seed copy stay tiny — the
    /// 119 MB / 17 924 files behind the node_modules links must never be materialised again.
    #[test]
    fn fresh_project_seed_copy_stays_small() {
        let root = std::env::temp_dir().join(format!("arbos-seed-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let seed = root.join("seed").join("extensions").join("ext");
        let shared = root.join("global").join("pkg");
        std::fs::create_dir_all(seed.join("node_modules")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        for i in 0..200 {
            std::fs::write(shared.join(format!("f{i}.js")), vec![b'x'; 16 * 1024]).unwrap();
        }
        std::fs::write(seed.join("index.ts"), "export {}").unwrap();
        std::os::unix::fs::symlink(&shared, seed.join("node_modules").join("pkg")).unwrap();
        let ws = root.join("fresh-project");
        copy_dir(&root.join("seed"), &ws).unwrap();
        let (bytes, files) = own(&ws);
        assert!(bytes < 64 * 1024, "fresh project holds {bytes} own bytes");
        assert!(files < 16, "fresh project holds {files} own files");
        assert!(
            ws.join("extensions/ext/node_modules/pkg/f0.js").exists(),
            "link resolves"
        );
        let _ = std::fs::remove_dir_all(&root);

        /// Bytes and files really under `p` (symlinks counted as one entry, not followed).
        fn own(p: &std::path::Path) -> (u64, u64) {
            let (mut b, mut n) = (0, 0);
            for e in std::fs::read_dir(p).unwrap().flatten() {
                let m = std::fs::symlink_metadata(e.path()).unwrap();
                n += 1;
                if m.file_type().is_symlink() {
                    continue;
                }
                if m.is_dir() {
                    let (sb, sn) = own(&e.path());
                    b += sb;
                    n += sn;
                } else {
                    b += m.len();
                }
            }
            (b, n)
        }
    }

    /// An image roll must reach existing workspaces: a seed file whose bytes changed is copied
    /// again, an unchanged one is left alone, a missing one is created.
    #[test]
    fn seed_copy_refreshes_changed_files() {
        let root = std::env::temp_dir().join(format!("arbos-seed-{}", std::process::id()));
        let (seed, dst) = (root.join("seed").join("ext"), root.join("dst").join("ext"));
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join("index.ts"), "v1").unwrap();
        std::fs::write(seed.join("deny.ts"), "rules").unwrap();
        copy_dir(&root.join("seed"), &root.join("dst")).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("index.ts")).unwrap(), "v1");
        // The image rolls: index.ts changes, deny.ts does not, a new file appears.
        std::fs::write(seed.join("index.ts"), "v2 with the new guardrails").unwrap();
        std::fs::write(seed.join("route-schema.ts"), "new").unwrap();
        copy_dir(&root.join("seed"), &root.join("dst")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dst.join("index.ts")).unwrap(),
            "v2 with the new guardrails"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("deny.ts")).unwrap(),
            "rules"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("route-schema.ts")).unwrap(),
            "new"
        );
        assert!(!seed_file_differs(
            &seed.join("deny.ts"),
            &dst.join("deny.ts")
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #1257: a session fork copies the transcript under the new chat id, leaves nothing for
    /// the agent loop to re-run, and hands pi the same entries under the new session id.
    #[test]
    fn forked_doc_keeps_messages_under_the_new_chat_and_runs_nothing_again() {
        let src = ChatDoc::new(Some(7), true);
        src.init_meta("c-unicorns", "p-bob", 1);
        src.append_user_message(
            "m1",
            "hello unicorns",
            "dev-mac",
            &arbos_core::doc::UserMessageOpts::default(),
        )
        .expect("message");
        let cmd = src.push_command("send", json!({ "messageId": "m1" }), "dev-mac");
        let snap = src.snapshot();

        let forked = fork_doc_snapshot(&snap, 9, "c-fork", "p-bob").expect("fork");
        let doc = ChatDoc::from_snapshot(Some(9), true, &forked).expect("load");
        assert_eq!(doc.meta()["chatId"], json!("c-fork"));
        assert_eq!(doc.meta()["projectId"], json!("p-bob"));
        let msgs = doc.messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text(), "hello unicorns");
        let cmds = doc.commands();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].id, cmd);
        assert_eq!(
            cmds[0].status, "done",
            "the fork never replays the source's commands"
        );
        assert!(doc.pending_commands().is_empty());
        // The source is untouched.
        assert_eq!(src.meta()["chatId"], json!("c-unicorns"));
        assert_eq!(src.pending_commands().len(), 1);
    }

    #[test]
    fn forked_session_text_renames_only_the_header() {
        let text = concat!(
            "{\"type\":\"session\",\"version\":3,\"id\":\"c-unicorns\",\"timestamp\":\"t\",\"cwd\":\"/work\"}\n",
            "{\"type\":\"message\",\"id\":\"e1\",\"role\":\"user\",\"content\":\"hello\"}\n",
            "{\"type\":\"message\",\"id\":\"e2\",\"role\":\"assistant\",\"content\":\"hi\"}\n",
        );
        let out = fork_session_text(text, "c-fork");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        let header: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["id"], json!("c-fork"));
        assert_eq!(header["type"], json!("session"));
        assert_eq!(header["cwd"], json!("/work"));
        assert_eq!(lines[1], text.lines().nth(1).unwrap());
        assert_eq!(lines[2], text.lines().nth(2).unwrap());
    }
}
