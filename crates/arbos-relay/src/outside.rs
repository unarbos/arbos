//! Outside agents (2026-10-28, #2140; design `docs/outside-agents-spec.md` in the store): an
//! agent whose brain runs somewhere Arbos does not control — Jacob's Grok Bot, Claude Code on a
//! laptop, a custom bot — joins chats as a member the **relay hosts**. It is a project row with
//! no box and one `arbk_…` key (only the hash is stored). The relay is its Loro peer: it writes
//! the agent's rows through [`crate::notices::write_doc`] and hands the agent every announced
//! message of its chats as Arbos Session Protocol v1 events, over HTTP long-poll or a WebSocket.
//! The owner's own box never learns the project exists ([`served`] filters it out of `/box/*`),
//! so an outside agent needs no agentd change anywhere.
//!
//! Owner side (device bearer): `POST /outside-agents`, `GET /outside-agents`,
//! `GET /outside-agents/{id}`, `POST /outside-agents/{id}/rotate`, `DELETE /outside-agents/{id}`.
//! Agent side (bearer `arbk_…`): `GET /asp/me`, `GET /asp/inbox?since=&wait=`, `WS /asp/socket`,
//! `POST /asp/events` (`message.post` · `message.start` · `message.delta` · `message.end` ·
//! `typing` · `status` · `presence.heartbeat`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use arbos_core::auth::hash;
use arbos_core::doc::{ChatDoc, CommandStatus, MessageStatus};
use arbos_core::ids::{now_ms, ulid};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{SinkExt, StreamExt};
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::api::{internal, ApiResult};
use crate::auth::Principal;
use crate::db::{Chat, Project};
use crate::notify::{ChatRef, Sender};
use crate::people;
use crate::signin::api_error;
use crate::state::AppState;

/// Protocol the relay speaks to outside agents.
pub const PROTOCOL: &str = "asp/1";
/// Key prefix; the rest is 40 hex characters (160 bits).
pub const KEY_PREFIX: &str = "arbk_";
/// Presence lease: an agent seen within this window is "connected" (Buzz: 3 × a 60 s heartbeat).
pub const ONLINE_MS: i64 = 180_000;
/// Longest a `GET /asp/inbox?wait=` call parks.
const WAIT_MAX_S: u64 = 25;
/// Events per agent per minute, every kind together.
const EVENTS_PER_MIN: i64 = 240;
/// Longest message text an outside agent may post.
const TEXT_MAX: usize = 16 * 1024;
/// Transports the owner can pick; the value is informational (shown on the profile).
const TRANSPORTS: &[&str] = &["acp", "grok", "custom"];

// ---- schema --------------------------------------------------------------------------------------

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS outside_agents (
  project_id TEXT PRIMARY KEY, user_id TEXT NOT NULL, transport TEXT NOT NULL,
  key_hash TEXT NOT NULL UNIQUE, created_at INTEGER NOT NULL, revoked_at INTEGER,
  rotated_at INTEGER, last_seen_at INTEGER, connected_at INTEGER,
  agent_info TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS asp_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL, created_at INTEGER NOT NULL,
  payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS asp_events_project ON asp_events(project_id, id);
"#;

/// One outside agent row.
#[derive(Debug, Clone)]
pub struct OutsideAgent {
    pub project_id: String,
    pub user_id: String,
    pub transport: String,
    pub created_at: i64,
    pub revoked_at: Option<i64>,
    pub rotated_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub connected_at: Option<i64>,
    pub agent_info: Value,
}

impl OutsideAgent {
    pub fn connected(&self) -> bool {
        self.revoked_at.is_none() && self.last_seen_at.is_some_and(|t| now_ms() - t <= ONLINE_MS)
    }

    /// `connected` | `away` | `never` | `revoked`.
    pub fn status(&self) -> &'static str {
        if self.revoked_at.is_some() {
            "revoked"
        } else if self.connected() {
            "connected"
        } else if self.last_seen_at.is_some() {
            "away"
        } else {
            "never"
        }
    }

    pub fn json(&self, project: Option<&Project>) -> Value {
        json!({
            "project_id": self.project_id,
            "name": project.map(|p| p.name.clone()),
            "handle": project.map(|p| people::agent_slug(&p.name)),
            "transport": self.transport,
            "protocol": PROTOCOL,
            "status": self.status(),
            "connected": self.connected(),
            "created_at": self.created_at,
            "revoked_at": self.revoked_at,
            "rotated_at": self.rotated_at,
            "last_seen_at": self.last_seen_at,
            "connected_at": self.connected_at,
            "agent": self.agent_info,
        })
    }
}

fn row_to_agent(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutsideAgent> {
    Ok(OutsideAgent {
        project_id: r.get(0)?,
        user_id: r.get(1)?,
        transport: r.get(2)?,
        created_at: r.get(3)?,
        revoked_at: r.get(4)?,
        rotated_at: r.get(5)?,
        last_seen_at: r.get(6)?,
        connected_at: r.get(7)?,
        agent_info: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or(Value::Null),
    })
}

const AGENT_COLS: &str = "project_id, user_id, transport, created_at, revoked_at, rotated_at, last_seen_at, connected_at, agent_info";

impl crate::db::Db {
    pub fn outside_agent(&self, project_id: &str) -> anyhow::Result<Option<OutsideAgent>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {AGENT_COLS} FROM outside_agents WHERE project_id = ?1"),
                params![project_id],
                row_to_agent,
            )
            .optional()
        })
    }

    pub fn outside_agents_of_user(&self, user_id: &str) -> anyhow::Result<Vec<OutsideAgent>> {
        self.with(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {AGENT_COLS} FROM outside_agents WHERE user_id = ?1 ORDER BY created_at DESC"
            ))?;
            let rows = st.query_map(params![user_id], row_to_agent)?;
            rows.collect()
        })
    }

    pub fn outside_agent_by_key_hash(&self, h: &str) -> anyhow::Result<Option<OutsideAgent>> {
        self.with(|c| {
            c.query_row(
                &format!("SELECT {AGENT_COLS} FROM outside_agents WHERE key_hash = ?1 AND revoked_at IS NULL"),
                params![h],
                row_to_agent,
            )
            .optional()
        })
    }

    /// Every outside project id (small table; cached callers filter lists with it).
    pub fn outside_project_ids(&self) -> anyhow::Result<Vec<String>> {
        self.with(|c| {
            let mut st = c.prepare("SELECT project_id FROM outside_agents")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
    }

    pub fn is_outside_project(&self, project_id: &str) -> bool {
        self.with(|c| {
            c.query_row(
                "SELECT 1 FROM outside_agents WHERE project_id = ?1",
                params![project_id],
                |_| Ok(()),
            )
            .optional()
        })
        .ok()
        .flatten()
        .is_some()
    }

    pub fn insert_outside_agent(
        &self,
        project_id: &str,
        user_id: &str,
        transport: &str,
        key_hash: &str,
    ) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO outside_agents(project_id, user_id, transport, key_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![project_id, user_id, transport, key_hash, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn rotate_outside_key(&self, project_id: &str, key_hash: &str) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE outside_agents SET key_hash = ?2, rotated_at = ?3, revoked_at = NULL, last_seen_at = NULL, connected_at = NULL WHERE project_id = ?1",
                params![project_id, key_hash, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn revoke_outside_agent(&self, project_id: &str) -> anyhow::Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE outside_agents SET revoked_at = ?2 WHERE project_id = ?1 AND revoked_at IS NULL",
                params![project_id, now_ms()],
            )?;
            Ok(())
        })
    }

    /// The agent was heard from: renew the lease; the first time also stamps `connected_at`.
    pub fn touch_outside_agent(
        &self,
        project_id: &str,
        info: Option<&Value>,
    ) -> anyhow::Result<()> {
        self.with(|c| {
            let now = now_ms();
            c.execute(
                "UPDATE outside_agents SET last_seen_at = ?2, connected_at = COALESCE(connected_at, ?2) WHERE project_id = ?1",
                params![project_id, now],
            )?;
            if let Some(i) = info.filter(|i| i.is_object()) {
                c.execute(
                    "UPDATE outside_agents SET agent_info = ?2 WHERE project_id = ?1",
                    params![project_id, i.to_string()],
                )?;
            }
            Ok(())
        })
    }

    pub fn insert_asp_event(&self, project_id: &str, payload: &Value) -> anyhow::Result<i64> {
        self.with(|c| {
            c.execute(
                "INSERT INTO asp_events(project_id, created_at, payload) VALUES (?1, ?2, ?3)",
                params![project_id, now_ms(), payload.to_string()],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn asp_events_after(
        &self,
        project_id: &str,
        after: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<(i64, Value)>> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT id, payload FROM asp_events WHERE project_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
            )?;
            let rows = st.query_map(params![project_id, after, limit], |r| {
                let id: i64 = r.get(0)?;
                let payload: String = r.get(1)?;
                Ok((id, serde_json::from_str(&payload).unwrap_or(Value::Null)))
            })?;
            rows.collect()
        })
    }

    pub fn asp_events_head(&self, project_id: &str) -> anyhow::Result<i64> {
        self.with(|c| {
            c.query_row(
                "SELECT COALESCE(MAX(id), 0) FROM asp_events WHERE project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )
        })
    }

    /// Events older than `keep_ms` are gone (the replay window).
    pub fn prune_asp_events(&self, keep_ms: i64) -> anyhow::Result<usize> {
        self.with(|c| {
            c.execute(
                "DELETE FROM asp_events WHERE created_at < ?1",
                params![now_ms() - keep_ms],
            )
        })
    }
}

// ---- helpers used by the rest of the relay -----------------------------------------------------

/// Drop outside projects from a list meant for a box (`/box/hello`, `/box/projects`, the box's
/// `GET /projects`): the owner's box must never make a workspace for an agent the relay hosts.
pub fn served(state: &AppState, projects: Vec<Project>) -> Vec<Project> {
    let outside = state.db.outside_project_ids().unwrap_or_default();
    if outside.is_empty() {
        return projects;
    }
    projects
        .into_iter()
        .filter(|p| !outside.contains(&p.id))
        .collect()
}

pub fn is_outside(state: &AppState, project_id: &str) -> bool {
    state.db.is_outside_project(project_id)
}

/// Mint a key; only its hash is stored.
pub fn mint_key() -> String {
    let mut bytes = [0u8; 20];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    format!("{KEY_PREFIX}{}", hex::encode(bytes))
}

fn waiters() -> &'static StdMutex<HashMap<String, Arc<Notify>>> {
    static W: OnceLock<StdMutex<HashMap<String, Arc<Notify>>>> = OnceLock::new();
    W.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn waiter(project_id: &str) -> Arc<Notify> {
    waiters()
        .lock()
        .unwrap()
        .entry(project_id.to_owned())
        .or_insert_with(|| Arc::new(Notify::new()))
        .clone()
}

/// Open streaming rows per `(project, chat)`: the message id the next `message.delta` without
/// one goes to, and the user message it answers (to finish its `send` command).
type OpenStreams = HashMap<(String, String), (String, Option<String>)>;

fn open_streams() -> &'static StdMutex<OpenStreams> {
    static S: OnceLock<StdMutex<OpenStreams>> = OnceLock::new();
    S.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// Store one event for the agent and wake whoever is parked on its inbox.
fn deliver(state: &AppState, project_id: &str, mut payload: Value) {
    payload["ts"] = json!(now_ms());
    match state.db.insert_asp_event(project_id, &payload) {
        Ok(id) => {
            tracing::debug!(project = %project_id, id, kind = %payload["type"], "asp event queued");
            waiter(project_id).notify_waiters();
        }
        Err(e) => tracing::warn!(project = %project_id, error = %e, "asp event not stored"),
    }
}

/// `@handle` of a project, as the apps mention it.
fn handle_of(p: &Project) -> String {
    people::agent_slug(&p.name)
}

fn mentions_agent(text: &str, mentions: &[String], project: &Project) -> bool {
    let me = format!("agent:{}", project.id);
    if mentions.iter().any(|m| m == &me) {
        return true;
    }
    let handle = handle_of(project);
    if handle.is_empty() {
        return false;
    }
    let needle = format!("@{handle}");
    let lower = text.to_ascii_lowercase();
    lower
        .match_indices(&needle.to_ascii_lowercase())
        .any(|(i, _)| {
            lower[i + needle.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'))
        })
}

/// Called from `notify::announce_with` once per announced message (people and project chats):
/// every outside agent in the chat gets a `message.received` event. `wake` says whether the
/// agent should answer — always in its own 1:1 chat and in DMs; in groups when it is
/// mentioned or the message replies to it (v1 rule; the jev judge for outside agents follows).
pub fn on_announce(
    state: &AppState,
    chat: &ChatRef,
    sender: &Sender,
    message_id: &str,
    text: &str,
    reply_to: Option<&str>,
    mentions: &[String],
) {
    let targets: Vec<(Project, bool, Value)> = match &chat.project_id {
        Some(pid) => {
            if !is_outside(state, pid) {
                return;
            }
            let Ok(Some(p)) = state.db.project(pid) else {
                return;
            };
            let title = state
                .db
                .chat(pid, &chat.chat_id)
                .ok()
                .flatten()
                .map(|c| c.title)
                .unwrap_or_default();
            vec![(
                p,
                true,
                json!({ "id": chat.chat_id, "kind": "agent", "project_id": pid, "title": title }),
            )]
        }
        None => {
            let Ok(cm) = people::load_chat(state, &chat.chat_id) else {
                return;
            };
            let kind = cm.chat.kind.clone();
            let title = cm.chat.title.clone().unwrap_or_default();
            cm.members
                .iter()
                .filter_map(|m| m.project_id().map(str::to_owned))
                .filter(|pid| is_outside(state, pid))
                .filter_map(|pid| state.db.project(&pid).ok().flatten())
                .map(|p| {
                    let wake = kind == "dm" || mentions_agent(text, mentions, &p);
                    (
                        p,
                        wake,
                        json!({ "id": chat.chat_id, "kind": kind, "project_id": Value::Null, "title": title }),
                    )
                })
                .collect()
        }
    };
    // Remember the id: a row fallback racing this announce must not deliver it again.
    let _ = mark_seen(&chat.chat_id, message_id);
    for (p, wake, chat_json) in targets {
        // Its own message, announced by the relay on its behalf: not news to it.
        if sender.kind == "agent" && sender.id == p.id {
            continue;
        }
        if state
            .db
            .outside_agent(&p.id)
            .ok()
            .flatten()
            .is_none_or(|a| a.revoked_at.is_some())
        {
            continue;
        }
        deliver(
            state,
            &p.id,
            json!({
                "type": "message.received",
                "chat": chat_json,
                "message": {
                    "id": message_id,
                    "text": text,
                    "from": {
                        "kind": sender.kind,
                        "id": sender.id,
                        "name": sender.name,
                        "handle": sender.handle,
                    },
                    "reply_to": reply_to,
                    "mentions": mentions,
                },
                "wake": wake,
            }),
        );
    }
}

/// Message ids already handed to outside agents by the row fallback (per chat, newest 64), so
/// a row and its late announce never deliver the same message twice.
fn seen_rows() -> &'static StdMutex<HashMap<String, std::collections::VecDeque<String>>> {
    static S: OnceLock<StdMutex<HashMap<String, std::collections::VecDeque<String>>>> =
        OnceLock::new();
    S.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn mark_seen(chat_id: &str, message_id: &str) -> bool {
    let mut all = seen_rows().lock().unwrap();
    let q = all.entry(chat_id.to_owned()).or_default();
    if q.iter().any(|m| m == message_id) {
        return false;
    }
    q.push_back(message_id.to_owned());
    while q.len() > 64 {
        q.pop_front();
    }
    true
}

/// Does this chat have an outside agent in it (cheap enough for the row path)?
fn has_outside_member(state: &AppState, chat: &ChatRef) -> bool {
    match &chat.project_id {
        Some(pid) => is_outside(state, pid),
        None => people::load_chat(state, &chat.chat_id)
            .map(|cm| {
                cm.members
                    .iter()
                    .filter_map(|m| m.project_id())
                    .any(|pid| is_outside(state, pid))
            })
            .unwrap_or(false),
    }
}

/// A device wrote a row into a chat with an outside agent. The apps announce the message a
/// moment later (`POST …/notify`, which reaches [`on_announce`]); a client that does not
/// (arbosctl, older builds) would leave the agent deaf, so after a short wait the relay reads
/// the new user messages out of the doc itself and delivers them. An announce that arrives
/// first wins; the fallback never delivers a message twice.
pub fn on_row(state: &AppState, chat: ChatRef, user_id: &str, at: i64) {
    if !has_outside_member(state, &chat) {
        return;
    }
    let state = state.clone();
    let user_id = user_id.to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        if state
            .db
            .chat_event_since(&chat.chat_id, &user_id, at - 1_000)
            .unwrap_or(true)
        {
            // Announced: the apps' path handled it. Remember the ids so a later row of the
            // same message cannot double-deliver either.
            return;
        }
        let Ok(Some(doc)) =
            crate::matrix::backfill::load_doc(&state, &chat.chat_id, chat.project_id.as_deref())
        else {
            return;
        };
        let sender = Sender::user(&state, &user_id);
        let me = format!("user:{user_id}");
        for m in doc.messages() {
            if m.role != "user" {
                continue;
            }
            let created = m.rest["createdAt"].as_i64().unwrap_or(0);
            if created < at - 30_000 {
                continue;
            }
            // People chats stamp the author; a project chat's user rows are the owner's.
            let author = m.rest["sender"].as_str().unwrap_or("user");
            if author != "user" && author != me {
                continue;
            }
            let text = m.text();
            if text.trim().is_empty() || !mark_seen(&chat.chat_id, &m.id) {
                continue;
            }
            let mentions: Vec<String> = m.rest["mentions"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let reply_to = m.rest["replyTo"].as_str().map(str::to_owned);
            tracing::info!(chat = %chat.chat_id, message = %m.id, "row fallback: delivering to outside agents");
            on_announce(
                &state,
                &chat,
                &sender,
                &m.id,
                &text,
                reply_to.as_deref(),
                &mentions,
            );
        }
    });
}

/// People chats this agent sits in: `member.added` / `member.removed` keep the bridge's list fresh.
pub fn on_membership(state: &AppState, project_id: &str, chat_id: &str, added: bool) {
    if !is_outside(state, project_id) {
        return;
    }
    let (kind, title) = people::load_chat(state, chat_id)
        .map(|cm| {
            (
                cm.chat.kind.clone(),
                cm.chat.title.clone().unwrap_or_default(),
            )
        })
        .unwrap_or_else(|_| ("group".to_owned(), String::new()));
    deliver(
        state,
        project_id,
        json!({
            "type": if added { "member.added" } else { "member.removed" },
            "chat": { "id": chat_id, "kind": kind, "project_id": Value::Null, "title": title },
        }),
    );
}

// ---- owner API -----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub avatar_color_index: Option<i64>,
}

fn owner_of(principal: &Principal) -> Result<String, (StatusCode, String)> {
    match principal {
        Principal::Device { user_id, .. } => Ok(user_id.clone()),
        Principal::Box { .. } | Principal::Admin => {
            Err((StatusCode::FORBIDDEN, "devices only".into()))
        }
    }
}

fn link_for(state: &AppState, project_id: &str) -> String {
    format!(
        "{}/asp/connect/{project_id}",
        state.cfg.public_url.trim_end_matches('/')
    )
}

/// `POST /outside-agents {name, transport}` → the new project (an agent with no box) and its
/// key, shown once.
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    Json(b): Json<CreateBody>,
) -> ApiResult {
    let owner = owner_of(&principal)?;
    let name = b.name.trim().to_owned();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name required".into()));
    }
    let transport = b.transport.as_deref().unwrap_or("custom").to_owned();
    if !TRANSPORTS.contains(&transport.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            "transport must be acp|grok|custom".into(),
        ));
    }
    if let Some(i) = b.avatar_color_index {
        if !(0..7).contains(&i) {
            return Err((
                StatusCode::BAD_REQUEST,
                "avatar_color_index must be 0–6 or null".into(),
            ));
        }
    }
    let (name, _) = people::agent_name_clear_of_people(&state, &name);
    if state
        .db
        .project_of_user_by_name(&owner, &name)
        .map_err(internal)?
        .is_some()
    {
        return Err((
            StatusCode::CONFLICT,
            format!("you already have an agent named \"{name}\""),
        ));
    }
    let now = now_ms();
    let id = ulid();
    let (tree, color) =
        crate::avatar::default_pair(&id, &state.cfg.avatar_slugs, &state.cfg.avatar_colors, &[]);
    let project = Project {
        id: id.clone(),
        name: name.clone(),
        icon: name
            .chars()
            .next()
            .map(|c| c.to_string())
            .unwrap_or_default(),
        instructions: String::new(),
        models: json!({}),
        state: "hibernated_light".into(),
        created_at: now,
        last_active_at: now,
        spend_cap_daily: crate::credits::PROJECT_CAP_UNLIMITED,
        color: "#8b7cf6".into(),
        user_id: owner.clone(),
        workspace_state: "stopped".into(),
        avatar: Some(tree),
        avatar_color: Some(color),
        description: format!("Connected agent ({transport})"),
        primary_chat_id: None,
        avatar_color_index: b.avatar_color_index,
        // The Create sheets' rule (Jacob, 2026-10-25): the name's initial on a neutral disc,
        // never an automatic leaf; a picture can be set from the profile like any agent.
        avatar_leaf: Some(i64::from(arbos_core::avatar::leaf::NEUTRAL)),
        avatar_leaf_color: Some(i64::from(arbos_core::avatar::leaf::NEUTRAL)),
        image_key: None,
        avatar_version: 0,
    };
    state
        .db
        .insert_project(&project)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))?;
    let pinned = Chat {
        id: ulid(),
        project_id: project.id.clone(),
        title: name.clone(),
        is_pinned: true,
        position: 0,
        created_at: now,
        doc_epoch: 1,
        kind: "chat".into(),
        participants: Vec::new(),
        forked_from: None,
    };
    state.db.insert_chat(&pinned).map_err(internal)?;
    let _ = state.db.set_primary_chat(&project.id, Some(&pinned.id));
    let key = mint_key();
    state
        .db
        .insert_outside_agent(&project.id, &owner, &transport, &hash(&key))
        .map_err(internal)?;
    tracing::info!(project = %project.id, owner = %owner, %transport, "outside agent created");
    let agent = state
        .db
        .outside_agent(&project.id)
        .map_err(internal)?
        .ok_or_else(|| internal("row vanished"))?;
    Ok(Json(json!({
        "agent": agent.json(Some(&project)),
        "project": crate::api::project_json(&state, &project).await,
        "chat_id": pinned.id,
        "key": key,
        "link": link_for(&state, &project.id),
        "relay_url": state.cfg.public_url,
    }))
    .into_response())
}

/// `GET /outside-agents` — the caller's connected agents with their status.
pub async fn list(State(state): State<AppState>, principal: Principal) -> ApiResult {
    let owner = owner_of(&principal)?;
    let mut out = Vec::new();
    for a in state.db.outside_agents_of_user(&owner).map_err(internal)? {
        let p = state.db.project(&a.project_id).ok().flatten();
        out.push(a.json(p.as_ref()));
    }
    Ok(Json(json!({ "agents": out })).into_response())
}

fn owned(
    state: &AppState,
    principal: &Principal,
    project_id: &str,
) -> Result<(OutsideAgent, Project), (StatusCode, String)> {
    let owner = owner_of(principal)?;
    let agent = state
        .db
        .outside_agent(project_id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such connected agent".to_owned()))?;
    if agent.user_id != owner {
        return Err((StatusCode::FORBIDDEN, "not your agent".into()));
    }
    let project = state
        .db
        .project(project_id)
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such agent".to_owned()))?;
    Ok((agent, project))
}

/// `GET /outside-agents/{id}` — status (connected · away · never · revoked), transport, protocol.
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let (agent, project) = owned(&state, &principal, &id)?;
    Ok(Json(json!({
        "agent": agent.json(Some(&project)),
        "link": link_for(&state, &project.id),
        "relay_url": state.cfg.public_url,
    }))
    .into_response())
}

/// `POST /outside-agents/{id}/rotate` — a new key, shown once; the old one stops at once.
pub async fn rotate(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let (_, project) = owned(&state, &principal, &id)?;
    let key = mint_key();
    state
        .db
        .rotate_outside_key(&project.id, &hash(&key))
        .map_err(internal)?;
    waiter(&project.id).notify_waiters();
    let agent = state
        .db
        .outside_agent(&project.id)
        .map_err(internal)?
        .ok_or_else(|| internal("row vanished"))?;
    tracing::info!(project = %project.id, "outside agent key rotated");
    Ok(Json(json!({
        "agent": agent.json(Some(&project)),
        "key": key,
        "link": link_for(&state, &project.id),
        "relay_url": state.cfg.public_url,
    }))
    .into_response())
}

/// `DELETE /outside-agents/{id}` — revoke: the key dies, the agent shows as disconnected, its
/// messages stay. Deleting the agent itself is the ordinary project delete.
pub async fn revoke(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let (_, project) = owned(&state, &principal, &id)?;
    state
        .db
        .revoke_outside_agent(&project.id)
        .map_err(internal)?;
    waiter(&project.id).notify_waiters();
    tracing::info!(project = %project.id, "outside agent revoked");
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---- agent API -----------------------------------------------------------------------------------

/// The authenticated outside agent (bearer `arbk_…`).
#[derive(Debug, Clone)]
pub struct AgentPrincipal {
    pub agent: OutsideAgent,
    pub project: Project,
}

impl FromRequestParts<AppState> for AgentPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let tok = crate::auth::token_from_parts(parts).unwrap_or_default();
        if !tok.starts_with(KEY_PREFIX) {
            return Err(api_error(
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "an agent key (arbk_…) is required",
            ));
        }
        let agent = state
            .db
            .outside_agent_by_key_hash(&hash(&tok))
            .ok()
            .flatten()
            .ok_or_else(|| {
                api_error(
                    StatusCode::UNAUTHORIZED,
                    "invalid_token",
                    "this agent key is not valid",
                )
            })?;
        let project = state
            .db
            .project(&agent.project_id)
            .ok()
            .flatten()
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "not_found", "agent is gone"))?;
        if !state
            .db
            .rate_check_and_note(
                &format!("asp:{}", agent.project_id),
                "asp",
                60_000,
                EVENTS_PER_MIN,
            )
            .unwrap_or(true)
        {
            return Err(api_error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "too many requests this minute",
            ));
        }
        let _ = state.db.touch_outside_agent(&agent.project_id, None);
        Ok(AgentPrincipal { agent, project })
    }
}

fn chat_json_for(state: &AppState, project: &Project, chat_id: &str) -> Option<Value> {
    if let Ok(Some(c)) = state.db.chat(&project.id, chat_id) {
        return Some(
            json!({ "id": c.id, "kind": "agent", "project_id": project.id, "title": c.title }),
        );
    }
    let cm = people::load_chat(state, chat_id).ok()?;
    let me = format!("agent:{}", project.id);
    cm.members.iter().any(|m| m.member == me).then(|| {
        json!({
            "id": cm.chat.id, "kind": cm.chat.kind, "project_id": Value::Null,
            "title": cm.chat.title.clone().unwrap_or_default(),
        })
    })
}

/// `GET /asp/me` — who the key is, and the chats it may speak in.
pub async fn me(State(state): State<AppState>, me: AgentPrincipal) -> Response {
    let mut chats = Vec::new();
    for c in state.db.list_chats(&me.project.id).unwrap_or_default() {
        chats.push(
            json!({ "id": c.id, "kind": "agent", "project_id": me.project.id, "title": c.title }),
        );
    }
    for cm in state
        .db
        .chats_of_member(&format!("agent:{}", me.project.id))
        .unwrap_or_default()
    {
        chats.push(json!({
            "id": cm.chat.id, "kind": cm.chat.kind, "project_id": Value::Null,
            "title": cm.chat.title.clone().unwrap_or_default(),
        }));
    }
    let owner = state.db.user_card(&me.agent.user_id).ok().flatten();
    Json(json!({
        "protocol": PROTOCOL,
        "agent": {
            "id": me.project.id,
            "name": me.project.name,
            "handle": handle_of(&me.project),
            "transport": me.agent.transport,
            "owner": owner.map(|o| json!({ "id": o.id, "name": o.name, "handle": o.handle })),
        },
        "chats": chats,
        "inbox_head": state.db.asp_events_head(&me.project.id).unwrap_or(0),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    pub since: i64,
    #[serde(default)]
    pub wait: u64,
    #[serde(default)]
    pub limit: Option<i64>,
}

fn events_json(events: Vec<(i64, Value)>, since: i64) -> Value {
    let next = events.last().map(|(id, _)| *id).unwrap_or(since);
    let list: Vec<Value> = events
        .into_iter()
        .map(|(id, mut e)| {
            e["id"] = json!(id);
            e
        })
        .collect();
    json!({ "events": list, "next": next })
}

/// `GET /asp/inbox?since=<id>&wait=<s>` — events after `since`; parks up to `wait` seconds (≤ 25)
/// when there are none yet. `next` is the cursor for the following call.
pub async fn inbox(
    State(state): State<AppState>,
    me: AgentPrincipal,
    Query(q): Query<InboxQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let wait = q.wait.min(WAIT_MAX_S);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(wait);
    loop {
        let events = state
            .db
            .asp_events_after(&me.project.id, q.since, limit)
            .unwrap_or_default();
        if !events.is_empty() || wait == 0 {
            return Json(events_json(events, q.since)).into_response();
        }
        let n = waiter(&me.project.id);
        let notified = n.notified();
        tokio::pin!(notified);
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(deadline) => {
                return Json(events_json(Vec::new(), q.since)).into_response();
            }
        }
    }
}

#[derive(Deserialize)]
pub struct EventsBody {
    #[serde(default)]
    pub events: Vec<Value>,
    #[serde(flatten)]
    pub one: Value,
}

/// `POST /asp/events` — one event (`{type, …}`) or `{events: […]}`; answers one result per event.
pub async fn events(
    State(state): State<AppState>,
    me: AgentPrincipal,
    Json(b): Json<EventsBody>,
) -> Response {
    let mut list = b.events;
    if list.is_empty() && b.one.get("type").is_some() {
        list.push(b.one);
    }
    if list.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "bad_request", "no event");
    }
    if list.len() > 50 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "at most 50 events per call",
        );
    }
    let mut results = Vec::new();
    for e in list {
        results.push(match apply_event(&state, &me, &e).await {
            Ok(v) => v,
            Err((code, msg)) => json!({ "ok": false, "error": code, "message": msg }),
        });
    }
    Json(json!({ "results": results })).into_response()
}

type EventResult = Result<Value, (&'static str, String)>;

fn field<'a>(e: &'a Value, k: &str) -> Option<&'a str> {
    e.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

/// The relay's device id on rows it writes for this agent.
fn device_of(project_id: &str) -> String {
    format!("outside:{project_id}")
}

/// Attribution fields a people-chat row carries (what agentd's `people_reply_fields` writes).
fn people_fields(
    state: &AppState,
    project: &Project,
    reply_to: Option<&str>,
) -> Vec<(&'static str, Value)> {
    let owner_handle = state
        .db
        .user_card(&project.user_id)
        .ok()
        .flatten()
        .map(|c| c.handle)
        .unwrap_or_default();
    let mut f = vec![
        ("sender", json!(format!("agent:{}", project.id))),
        ("senderHandle", json!(handle_of(project))),
        ("senderName", json!(project.name)),
        ("ownerHandle", json!(owner_handle)),
    ];
    if let Some(r) = reply_to {
        f.push(("inReplyTo", json!(r)));
        f.push(("replyTo", json!(r)));
    }
    f
}

/// Mark the `send` command of the user message this reply answers (project chats only).
fn settle_send(doc: &ChatDoc, reply_to: Option<&str>, status: CommandStatus) {
    for c in doc.commands() {
        if c.kind != "send" {
            continue;
        }
        let answers = reply_to.is_none_or(|r| c.payload["messageId"].as_str() == Some(r));
        let open = match status {
            CommandStatus::Taken => c.status == "pending",
            _ => c.status == "pending" || c.status == "taken",
        };
        if answers && open {
            doc.set_command_status(&c.id, status, None);
        }
    }
}

async fn apply_event(state: &AppState, me: &AgentPrincipal, e: &Value) -> EventResult {
    let ty = field(e, "type").ok_or(("bad_request", "type required".to_owned()))?;
    let pid = me.project.id.clone();
    match ty {
        "presence.heartbeat" => {
            let _ = state.db.touch_outside_agent(&pid, e.get("agent"));
            Ok(json!({ "ok": true, "type": ty }))
        }
        "typing" | "status" => {
            let chat_id = field(e, "chat").ok_or(("bad_request", "chat required".to_owned()))?;
            let chat = chat_json_for(state, &me.project, chat_id)
                .ok_or(("forbidden", "not a chat of this agent".to_owned()))?;
            let on = e
                .get("on")
                .and_then(|v| v.as_bool())
                .unwrap_or(ty == "status");
            let text = field(e, "text").map(str::to_owned);
            let open = open_streams()
                .lock()
                .unwrap()
                .get(&(pid.clone(), chat_id.to_owned()))
                .cloned();
            if let Some((msg_id, _)) = open {
                let project_id = chat["project_id"].as_str().map(str::to_owned);
                let activity = if on { text.clone() } else { None };
                let _ = crate::notices::write_doc_as(
                    state,
                    project_id.as_deref(),
                    chat_id,
                    &device_of(&pid),
                    |doc| {
                        doc.set_activity(&msg_id, activity.as_deref());
                        Ok(())
                    },
                )
                .await;
            }
            // The project-level activity record drives "<name> is …" in the 1:1 chat.
            let mut map = state.activity.lock().await;
            if on {
                map.insert(
                    pid.clone(),
                    crate::state::ProjectActivity {
                        chat_id: chat_id.to_owned(),
                        text: text
                            .unwrap_or_else(|| "typing".to_owned())
                            .chars()
                            .take(80)
                            .collect(),
                        at: now_ms(),
                        jobs: None,
                    },
                );
            } else {
                map.remove(&pid);
            }
            Ok(json!({ "ok": true, "type": ty }))
        }
        "message.post" | "message.start" => {
            let chat_id = field(e, "chat").ok_or(("bad_request", "chat required".to_owned()))?;
            let chat = chat_json_for(state, &me.project, chat_id)
                .ok_or(("forbidden", "not a chat of this agent".to_owned()))?;
            let text = e
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            if ty == "message.post" && text.trim().is_empty() {
                return Err(("bad_request", "text required".to_owned()));
            }
            if text.len() > TEXT_MAX {
                return Err(("too_large", "text over 16 KB".to_owned()));
            }
            let reply_to = field(e, "reply_to").map(str::to_owned);
            let project_id = chat["project_id"].as_str().map(str::to_owned);
            let is_people = project_id.is_none();
            let msg_id = ulid();
            let finished = ty == "message.post";
            let fields = if is_people {
                people_fields(state, &me.project, reply_to.as_deref())
            } else {
                Vec::new()
            };
            let (m, t, r) = (msg_id.clone(), text.clone(), reply_to.clone());
            let device = device_of(&pid);
            let device_in = device.clone();
            crate::notices::write_doc_as(
                state,
                project_id.as_deref(),
                chat_id,
                &device,
                move |doc| {
                    let part =
                        doc.start_assistant_message(&m, r.as_deref().unwrap_or(""), &device_in);
                    if !fields.is_empty() {
                        doc.set_message_fields(&m, &fields);
                    }
                    if !t.is_empty() {
                        doc.append_text(&m, &part, &t);
                    }
                    if !is_people {
                        settle_send(
                            doc,
                            r.as_deref(),
                            if finished {
                                CommandStatus::Done
                            } else {
                                CommandStatus::Taken
                            },
                        );
                    }
                    if finished {
                        doc.finish_message(&m, MessageStatus::Done);
                    }
                    Ok(())
                },
            )
            .await
            .map_err(|e| ("write_failed", e.to_string()))?;
            if finished {
                announce(state, &me.project, &chat, &msg_id, &text).await;
                state.activity.lock().await.remove(&pid);
            } else {
                open_streams().lock().unwrap().insert(
                    (pid.clone(), chat_id.to_owned()),
                    (msg_id.clone(), reply_to),
                );
            }
            Ok(json!({ "ok": true, "type": ty, "message": msg_id, "chat": chat_id }))
        }
        "message.delta" | "message.end" => {
            let chat_id = field(e, "chat").ok_or(("bad_request", "chat required".to_owned()))?;
            let chat = chat_json_for(state, &me.project, chat_id)
                .ok_or(("forbidden", "not a chat of this agent".to_owned()))?;
            let key = (pid.clone(), chat_id.to_owned());
            let open = open_streams().lock().unwrap().get(&key).cloned();
            let (msg_id, reply_to) = match (field(e, "message"), open) {
                (Some(m), Some((open_id, r))) if open_id == m => (open_id, r),
                (Some(m), _) => (m.to_owned(), None),
                (None, Some(o)) => o,
                (None, None) => {
                    return Err(("bad_request", "no open message in this chat".to_owned()));
                }
            };
            let text = e.get("text").and_then(|v| v.as_str()).map(str::to_owned);
            if text.as_ref().is_some_and(|t| t.len() > TEXT_MAX) {
                return Err(("too_large", "text over 16 KB".to_owned()));
            }
            let project_id = chat["project_id"].as_str().map(str::to_owned);
            let ending = ty == "message.end";
            let (m, t, r) = (msg_id.clone(), text.clone(), reply_to.clone());
            let is_people = project_id.is_none();
            let full = std::sync::Arc::new(StdMutex::new(String::new()));
            let full2 = full.clone();
            crate::notices::write_doc_as(
                state,
                project_id.as_deref(),
                chat_id,
                &device_of(&pid),
                move |doc| {
                    let part = doc
                        .part_id_at(&m, 0)
                        .ok_or_else(|| anyhow::anyhow!("no such message"))?;
                    match (ending, t.as_deref()) {
                        // A final text replaces whatever streamed (authoritative).
                        (true, Some(whole)) => doc.edit_message_text(&m, whole)?,
                        (_, Some(delta)) => doc.append_text(&m, &part, delta),
                        (_, None) => {}
                    }
                    if ending {
                        doc.set_activity(&m, None);
                        doc.finish_message(&m, MessageStatus::Done);
                        if !is_people {
                            settle_send(doc, r.as_deref(), CommandStatus::Done);
                        }
                        if let Some(row) = doc.message(&m) {
                            *full2.lock().unwrap() = row.text();
                        }
                    }
                    Ok(())
                },
            )
            .await
            .map_err(|e| ("write_failed", e.to_string()))?;
            if ending {
                open_streams().lock().unwrap().remove(&key);
                let whole = full.lock().unwrap().clone();
                announce(state, &me.project, &chat, &msg_id, &whole).await;
                state.activity.lock().await.remove(&pid);
            }
            Ok(json!({ "ok": true, "type": ty, "message": msg_id, "chat": chat_id }))
        }
        other => Err(("unknown_type", format!("unknown event type {other}"))),
    }
}

/// What agentd does after each finished message: announce it (pushes, Matrix mirror, badges).
async fn announce(state: &AppState, project: &Project, chat: &Value, msg_id: &str, text: &str) {
    let chat_id = chat["id"].as_str().unwrap_or_default();
    let chat_ref = match chat["project_id"].as_str() {
        Some(pid) => ChatRef::project(pid, chat_id),
        None => ChatRef::people(chat_id),
    };
    let sender = Sender::agent(state, project);
    let _ = crate::notify::announce_with(state, &chat_ref, &sender, msg_id, text, None, &[]).await;
}

/// `GET /asp/socket?since=<id>` — the same events as the inbox, pushed as they happen; text
/// frames the agent sends are events (`POST /asp/events` bodies) and get one result frame each.
pub async fn socket(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    me: AgentPrincipal,
    Query(q): Query<InboxQuery>,
) -> Response {
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| run_socket(socket, state, me, q.since))
}

async fn run_socket(socket: WebSocket, state: AppState, me: AgentPrincipal, mut cursor: i64) {
    let (mut sink, mut stream) = socket.split();
    let pid = me.project.id.clone();
    let hello = json!({
        "type": "hello", "protocol": PROTOCOL,
        "agent": { "id": pid, "name": me.project.name, "handle": handle_of(&me.project) },
        "since": cursor,
    });
    if sink
        .send(Message::Text(hello.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    let mut lease = tokio::time::interval(std::time::Duration::from_secs(45));
    loop {
        // Drain what is queued first.
        let events = state
            .db
            .asp_events_after(&pid, cursor, 100)
            .unwrap_or_default();
        for (id, mut e) in events {
            e["id"] = json!(id);
            cursor = id;
            if sink
                .send(Message::Text(e.to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
        if state
            .db
            .outside_agent(&pid)
            .ok()
            .flatten()
            .is_none_or(|a| a.revoked_at.is_some())
        {
            let _ = sink
                .send(Message::Text(
                    json!({ "type": "bye", "reason": "revoked" })
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
        let n = waiter(&pid);
        let notified = n.notified();
        tokio::pin!(notified);
        tokio::select! {
            _ = &mut notified => {}
            _ = lease.tick() => {
                let _ = state.db.touch_outside_agent(&pid, None);
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        let _ = state.db.touch_outside_agent(&pid, None);
                        let reply = match serde_json::from_str::<Value>(&t) {
                            Ok(e) => {
                                let ack = e.get("ack").cloned();
                                let mut r = match apply_event(&state, &me, &e).await {
                                    Ok(v) => v,
                                    Err((code, msg)) => json!({ "ok": false, "error": code, "message": msg }),
                                };
                                r["type"] = json!("result");
                                r["for"] = e.get("type").cloned().unwrap_or(Value::Null);
                                if let Some(a) = ack { r["ack"] = a; }
                                r
                            }
                            Err(err) => json!({ "type": "result", "ok": false, "error": "bad_json", "message": err.to_string() }),
                        };
                        if sink.send(Message::Text(reply.to_string().into())).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => return,
                }
            }
        }
    }
}

/// `GET /asp/connect/{id}` — the link on the reveal screen: a plain page that says what to do
/// with the key (no secret in it; the key never travels in a URL).
pub async fn connect_page(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let name = state
        .db
        .project(&id)
        .ok()
        .flatten()
        .filter(|_| state.db.is_outside_project(&id))
        .map(|p| p.name)
        .unwrap_or_else(|| "this agent".to_owned());
    let relay = state.cfg.public_url.trim_end_matches('/').to_owned();
    let body = format!(
        "Connect {name} to Arbos\n\n\
         Relay:   {relay}\n\
         Agent:   {id}\n\n\
         Every call carries the key the Arbos app showed you once:  Authorization: Bearer arbk_…\n\n\
         Read:    GET  {relay}/asp/inbox?since=0&wait=25      (events; `next` is the next `since`)\n\
         Live:    WS   {relay}/asp/socket?since=0              (same events, pushed)\n\
         Write:   POST {relay}/asp/events  {{\"type\":\"message.post\",\"chat\":\"<chat id from the event>\",\"text\":\"…\",\"reply_to\":\"<message id>\"}}\n\
         Stream:  message.start → message.delta (text) → message.end\n\
         Typing:  {{\"type\":\"typing\",\"chat\":\"…\",\"on\":true}}   Status: {{\"type\":\"status\",\"chat\":\"…\",\"text\":\"reading Stripe\"}}\n\
         Alive:   {{\"type\":\"presence.heartbeat\"}} every 60 s (any call renews the lease)\n\n\
         Claude Code / Codex on a Mac: cargo install --git https://github.com/unarbos/arbos arbos-acp\n\
         then ARBOS_AGENT_KEY=arbk_… arbos-acp --relay {relay} --agent claude-agent-acp\n"
    );
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// Hourly: drop events past the replay window (7 days).
pub async fn spawn_sweeper(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Ok(n) = state.db.prune_asp_events(7 * 24 * 3600 * 1000) {
                if n > 0 {
                    tracing::info!(pruned = n, "asp events pruned");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> Project {
        Project {
            id: "p1".into(),
            name: name.into(),
            icon: String::new(),
            instructions: String::new(),
            models: json!({}),
            state: String::new(),
            created_at: 0,
            last_active_at: 0,
            spend_cap_daily: 0.0,
            color: String::new(),
            user_id: "u1".into(),
            workspace_state: String::new(),
            avatar: None,
            avatar_color: None,
            description: String::new(),
            primary_chat_id: None,
            avatar_color_index: None,
            avatar_leaf: None,
            avatar_leaf_color: None,
            image_key: None,
            avatar_version: 0,
        }
    }

    #[test]
    fn key_shape() {
        let k = mint_key();
        assert!(k.starts_with(KEY_PREFIX));
        assert_eq!(k.len(), KEY_PREFIX.len() + 40);
        assert_ne!(k, mint_key());
    }

    #[test]
    fn mention_rules() {
        let p = project("Arbos (Grok)");
        let handle = handle_of(&p);
        assert!(!handle.is_empty());
        assert!(mentions_agent(&format!("hey @{handle} go"), &[], &p));
        assert!(mentions_agent("", &["agent:p1".into()], &p));
        assert!(!mentions_agent(&format!("@{handle}x no"), &[], &p));
        assert!(!mentions_agent("nothing here", &["user:u9".into()], &p));
    }

    /// A person writes in the group and in the agent's own chat → one `message.received` each,
    /// `wake` by the v1 rule; the agent posts back → a finished assistant row in the doc, the
    /// box lists leave the project out, the key is single-use after rotate.
    #[tokio::test]
    async fn announce_in_and_post_out() {
        use crate::people::tests_support::{group, person, project_of};
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;

        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 7001);
        let (other, _) = person(&state, 7002);
        let pid = project_of(&state, &owner, "Grok Bot");
        let key = mint_key();
        state
            .db
            .insert_outside_agent(&pid, &owner, "acp", &hash(&key))
            .unwrap();
        let chat = Chat {
            id: ulid(),
            project_id: pid.clone(),
            title: "Grok Bot".into(),
            is_pinned: true,
            position: 0,
            created_at: now_ms(),
            doc_epoch: 1,
            kind: "chat".into(),
            participants: Vec::new(),
            forked_from: None,
        };
        state.db.insert_chat(&chat).unwrap();
        let g = group(&state, &[&owner, &other]);
        state
            .db
            .add_member(&g, &format!("agent:{pid}"), "agent", &owner, &json!({}))
            .unwrap();

        // The box lists must not carry it.
        let served = served(&state, state.db.projects_of_user(&owner).unwrap());
        assert!(served.iter().all(|p| p.id != pid));
        assert!(is_outside(&state, &pid));

        // 1:1 chat → wake; group without a mention → no wake; with @handle → wake.
        let me = Sender::user(&state, &owner);
        on_announce(
            &state,
            &ChatRef::project(&pid, &chat.id),
            &me,
            "m1",
            "hello there",
            None,
            &[],
        );
        on_announce(
            &state,
            &ChatRef::people(&g),
            &me,
            "m2",
            "just people talking",
            None,
            &[],
        );
        on_announce(
            &state,
            &ChatRef::people(&g),
            &me,
            "m3",
            "@grok-bot total please",
            None,
            &[],
        );
        let events = state.db.asp_events_after(&pid, 0, 10).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].1["type"], "message.received");
        assert_eq!(events[0].1["chat"]["kind"], "agent");
        assert_eq!(events[0].1["wake"], true);
        assert_eq!(events[1].1["wake"], false);
        assert_eq!(events[2].1["wake"], true);
        assert_eq!(events[2].1["message"]["text"], "@grok-bot total please");

        // Post a reply into the 1:1 chat and stream one into the group.
        let agent = state
            .db
            .outside_agent_by_key_hash(&hash(&key))
            .unwrap()
            .unwrap();
        let project = state.db.project(&pid).unwrap().unwrap();
        let me = AgentPrincipal { agent, project };
        let r = apply_event(
            &state,
            &me,
            &json!({ "type": "message.post", "chat": chat.id, "text": "Hi! 14 refunds.", "reply_to": "m1" }),
        )
        .await
        .unwrap();
        assert_eq!(r["ok"], true);
        let doc = crate::matrix::backfill::load_doc(&state, &chat.id, Some(&pid))
            .unwrap()
            .unwrap();
        let rows = doc.messages();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text(), "Hi! 14 refunds.");
        assert_eq!(rows[0].role, "assistant");
        assert_eq!(rows[0].status, "done");

        apply_event(&state, &me, &json!({ "type": "message.start", "chat": g }))
            .await
            .unwrap();
        apply_event(
            &state,
            &me,
            &json!({ "type": "message.delta", "chat": g, "text": "Working" }),
        )
        .await
        .unwrap();
        apply_event(
            &state,
            &me,
            &json!({ "type": "message.delta", "chat": g, "text": " on it." }),
        )
        .await
        .unwrap();
        apply_event(&state, &me, &json!({ "type": "message.end", "chat": g }))
            .await
            .unwrap();
        let doc = crate::matrix::backfill::load_doc(&state, &g, None)
            .unwrap()
            .unwrap();
        let rows = doc.messages();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text(), "Working on it.");
        assert_eq!(rows[0].rest["sender"], format!("agent:{pid}"));
        assert_eq!(rows[0].rest["senderName"], "Grok Bot");
        // Its own announced reply did not come back as an event.
        assert_eq!(state.db.asp_events_after(&pid, 0, 10).unwrap().len(), 3);

        // Another chat is refused; rotate kills the old key.
        let r = apply_event(
            &state,
            &me,
            &json!({ "type": "message.post", "chat": "nope", "text": "x" }),
        )
        .await;
        assert!(r.is_err());
        let fresh = mint_key();
        state.db.rotate_outside_key(&pid, &hash(&fresh)).unwrap();
        assert!(state
            .db
            .outside_agent_by_key_hash(&hash(&key))
            .unwrap()
            .is_none());
        assert!(state
            .db
            .outside_agent_by_key_hash(&hash(&fresh))
            .unwrap()
            .is_some());
    }

    #[test]
    fn status_words() {
        let mut a = OutsideAgent {
            project_id: "p1".into(),
            user_id: "u1".into(),
            transport: "acp".into(),
            created_at: 0,
            revoked_at: None,
            rotated_at: None,
            last_seen_at: None,
            connected_at: None,
            agent_info: Value::Null,
        };
        assert_eq!(a.status(), "never");
        a.last_seen_at = Some(now_ms());
        assert_eq!(a.status(), "connected");
        a.last_seen_at = Some(now_ms() - ONLINE_MS - 1);
        assert_eq!(a.status(), "away");
        a.revoked_at = Some(1);
        assert_eq!(a.status(), "revoked");
    }
}
