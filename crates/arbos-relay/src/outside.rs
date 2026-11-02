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
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::outside_wake::{self, LastWake, Nudge};
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
/// Longest a streamed message may grow, every delta together: the socket's frame cap.
const MESSAGE_MAX: usize = 64 * 1024;
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
/// Additive migration for rows made before the scopes column (run after [`SCHEMA`]).
pub const MIGRATIONS: &[&str] =
    &["ALTER TABLE outside_agents ADD COLUMN scopes TEXT NOT NULL DEFAULT '{}'"];

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
    /// Wake URL (`outside_wake`): the URL is plain, the header value never leaves the DB.
    pub wake_url: Option<String>,
    pub wake_auth_set: bool,
    pub wake_last: Option<LastWake>,
    /// Owner overrides per scope name (`{"files.read": true}`); [`Scopes::effective`] fills the rest.
    pub scopes: Value,
}

/// The nine permission switches (spec §4.3) with their defaults.
pub const SCOPES: &[(&str, bool)] = &[
    ("chat.read", true),
    ("chat.post", true),
    ("chat.react", true),
    ("presence", true),
    ("approval.request", true),
    ("secret.use", false),
    ("members.manage", false),
    ("files.read", false),
    ("calls.join", false),
];

impl OutsideAgent {
    /// Every scope with the owner's override applied over the default.
    pub fn effective_scopes(&self) -> serde_json::Map<String, Value> {
        let mut out = serde_json::Map::new();
        for (name, default) in SCOPES {
            let on = self
                .scopes
                .get(*name)
                .and_then(Value::as_bool)
                .unwrap_or(*default);
            out.insert((*name).to_owned(), Value::Bool(on));
        }
        out
    }

    pub fn allows(&self, scope: &str) -> bool {
        let default = SCOPES
            .iter()
            .find(|(n, _)| *n == scope)
            .map(|(_, d)| *d)
            .unwrap_or(false);
        self.scopes
            .get(scope)
            .and_then(Value::as_bool)
            .unwrap_or(default)
    }
}

impl OutsideAgent {
    /// What the owner API and the apps' Connection card see of the Wake URL (no secret).
    pub fn wake_json(&self) -> Value {
        json!({
            "url": self.wake_url,
            "url_set": self.wake_url.is_some(),
            "auth_set": self.wake_auth_set,
            "header": outside_wake::HEADER,
            "last": self.wake_last.as_ref().map(LastWake::json),
        })
    }

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
            "wake": self.wake_json(),
            "scopes": Value::Object(self.effective_scopes()),
        })
    }
}

fn row_to_agent(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutsideAgent> {
    let wake_last_at: Option<i64> = r.get(12)?;
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
        scopes: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_else(|_| json!({})),
        wake_url: r.get::<_, Option<String>>(10)?.filter(|u| !u.is_empty()),
        wake_auth_set: r
            .get::<_, Option<String>>(11)?
            .is_some_and(|a| !a.is_empty()),
        wake_last: wake_last_at.map(|at| LastWake {
            at,
            status: r.get::<_, Option<u16>>(13).unwrap_or(None),
            error: r.get::<_, Option<String>>(14).unwrap_or(None),
        }),
    })
}

const AGENT_COLS: &str = "project_id, user_id, transport, created_at, revoked_at, rotated_at, last_seen_at, connected_at, agent_info, scopes, wake_url, wake_auth, wake_last_at, wake_last_status, wake_last_error";

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

    /// Merge `patch` (scope name → bool) into the owner's overrides.
    pub fn set_outside_scopes(
        &self,
        project_id: &str,
        patch: &serde_json::Map<String, Value>,
    ) -> anyhow::Result<()> {
        let Some(agent) = self.outside_agent(project_id)? else {
            anyhow::bail!("no such connected agent");
        };
        let mut cur = agent.scopes.as_object().cloned().unwrap_or_default();
        for (k, v) in patch {
            if SCOPES.iter().any(|(n, _)| n == k) {
                if let Some(b) = v.as_bool() {
                    cur.insert(k.clone(), Value::Bool(b));
                }
            }
        }
        self.with(|c| {
            c.execute(
                "UPDATE outside_agents SET scopes = ?2 WHERE project_id = ?1",
                params![project_id, Value::Object(cur).to_string()],
            )?;
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

#[cfg(test)]
pub(crate) fn deliver_for_test(state: &AppState, project_id: &str, payload: Value) {
    deliver(state, project_id, payload);
}

/// Does this queued event ask the agent to act: a `wake: true` row or a chat it was added to?
fn wakes(payload: &Value) -> bool {
    payload["wake"].as_bool() == Some(true) || payload["type"] == "member.added"
}

/// Store one event for the agent, wake whoever is parked on its inbox, and nudge its Wake URL
/// when the event asks the agent to act.
fn deliver(state: &AppState, project_id: &str, mut payload: Value) {
    payload["ts"] = json!(now_ms());
    match state.db.insert_asp_event(project_id, &payload) {
        Ok(id) => {
            tracing::debug!(project = %project_id, id, kind = %payload["type"], "asp event queued");
            waiter(project_id).notify_waiters();
            if wakes(&payload) {
                outside_wake::nudge(
                    state,
                    Nudge {
                        agent: project_id.to_owned(),
                        chat: payload["chat"]["id"].as_str().map(str::to_owned),
                        event_id: id,
                        kind: payload["type"].as_str().unwrap_or_default().to_owned(),
                    },
                );
            }
        }
        Err(e) => tracing::warn!(project = %project_id, error = %e, "asp event not stored"),
    }
}

/// `@handle` of a project, as the apps mention it.
fn handle_of(p: &Project) -> String {
    people::agent_slug(&p.name)
}

/// Does this message wake the agent by naming it? An explicit `@handle` in the prose does, and
/// so does a declared `agent:<id>` the writer resolved — a chip with no handle in the sentence.
///
/// A collective (`@here`, `@agents`) does **not**, and that is a deliberate carve-out rather
/// than an oversight. `@here` names every agent in a chat (Jacob, 2026-10-24) and the event now
/// carries that expansion, as the row always has, so the array alone would wake every agent in
/// the room. For an outside agent the `wake` flag is the only signal it gets and it reads as
/// "answer this" rather than "you have a turn", so a single `@here` in a room with three agents
/// would produce three answers. Waking them is the right behaviour only once a turn and a
/// verdict are separate fields, which is an open protocol question; until it is answered the
/// collective names the agent in the event and leaves the wake where it was. An agent that
/// wants to act on one can read `mentions` itself.
fn mentions_agent(text: &str, mentions: &[String], project: &Project) -> bool {
    let lower = text.to_ascii_lowercase();
    let handle = handle_of(project);
    let tagged = !handle.is_empty() && {
        let needle = format!("@{handle}").to_ascii_lowercase();
        lower.match_indices(&needle).any(|(i, _)| {
            lower[i + needle.len()..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '-'))
        })
    };
    if tagged {
        return true;
    }
    // A collective in the prose is why this agent may be in `mentions` at all, and it is not a
    // reason to wake; a declared id without one still is.
    if people::names_a_collective(&lower) {
        return false;
    }
    mentions
        .iter()
        .any(|m| m == &format!("agent:{}", project.id))
}

/// How long an announce waits for its own row to reach the relay's doc. The apps send
/// `POST …/notify` and the room push side by side, so the announce often lands first
/// (Arbonites 01M64SV6F5…, #2305: the row was stored 440 ms after the announce).
const ROW_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
const ROW_POLL: std::time::Duration = std::time::Duration::from_millis(100);

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
    // Tests announce rows that are never stored and read the event at once.
    let wait = if cfg!(test) {
        std::time::Duration::ZERO
    } else {
        ROW_WAIT
    };
    on_announce_waiting(
        state, chat, sender, message_id, text, reply_to, mentions, wait,
    );
}

/// Why an announced message would wake one agent. `WhenMentioned` is answered from the message
/// body, so it is answered in [`deliver_announced`] once the stored row is in rather than at
/// announce time: the announce body is only the sending client's push preview (#2413).
#[derive(Clone, Copy)]
enum WakeRule {
    /// Its own chat, or a DM: every message in it is for the agent.
    Always,
    /// A group: only when the message tags the agent.
    WhenMentioned,
}

/// The event of one announced message, waiting for its row.
struct Announced {
    chat: ChatRef,
    sender: Sender,
    message_id: String,
    text: String,
    reply_to: Option<String>,
    mentions: Vec<String>,
    targets: Vec<(Project, WakeRule, Value)>,
}

/// [`on_announce`] that waits up to `wait` for the row when the relay's doc does not hold it
/// yet. A row that never shows up (not stored in time, or stored but not applicable to a
/// divergent doc) is delivered without `attachments` and with `attachments_unknown: true`:
/// an empty list would tell the agent the message has none.
#[allow(clippy::too_many_arguments)]
pub(crate) fn on_announce_waiting(
    state: &AppState,
    chat: &ChatRef,
    sender: &Sender,
    message_id: &str,
    text: &str,
    reply_to: Option<&str>,
    mentions: &[String],
    wait: std::time::Duration,
) {
    let targets: Vec<(Project, WakeRule, Value)> = match &chat.project_id {
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
                WakeRule::Always,
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
                    let rule = if kind == "dm" {
                        WakeRule::Always
                    } else {
                        WakeRule::WhenMentioned
                    };
                    (
                        p,
                        rule,
                        json!({ "id": chat.chat_id, "kind": kind, "project_id": Value::Null, "title": title }),
                    )
                })
                .collect()
        }
    };
    // Remember the id: a row fallback racing this announce must not deliver it again.
    let _ = mark_seen(&chat.chat_id, message_id);
    if targets.is_empty() {
        return;
    }
    let ev = Announced {
        chat: chat.clone(),
        sender: sender.clone(),
        message_id: message_id.to_owned(),
        text: text.to_owned(),
        reply_to: reply_to.map(str::to_owned),
        mentions: mentions.to_vec(),
        targets,
    };
    // Pictures, files, GIFs and chart PNGs come from the announced row itself (the announce
    // carries text only).
    let doc = load_announced_doc(state, chat);
    if doc
        .as_ref()
        .is_some_and(|d| d.message(message_id).is_some())
    {
        deliver_announced(state, &ev, doc.as_ref());
        return;
    }
    let handle = tokio::runtime::Handle::try_current()
        .ok()
        .filter(|_| !wait.is_zero());
    let Some(handle) = handle else {
        deliver_announced(state, &ev, doc.as_ref());
        return;
    };
    let state = state.clone();
    handle.spawn(async move {
        let room = arbos_core::wire::room_name(ev.chat.project_label(), &ev.chat.chat_id);
        let head = |s: &AppState| s.db.room_activity(&room).map(|(h, _)| h).unwrap_or(0);
        let deadline = tokio::time::Instant::now() + wait;
        let mut seen = head(&state);
        let mut doc = doc;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(ROW_POLL).await;
            let now = head(&state);
            if now == seen {
                continue;
            }
            seen = now;
            doc = load_announced_doc(&state, &ev.chat);
            if doc
                .as_ref()
                .is_some_and(|d| d.message(&ev.message_id).is_some())
            {
                break;
            }
        }
        deliver_announced(&state, &ev, doc.as_ref());
    });
}

fn load_announced_doc(state: &AppState, chat: &ChatRef) -> Option<ChatDoc> {
    crate::matrix::backfill::load_doc(state, &chat.chat_id, chat.project_id.as_deref())
        .ok()
        .flatten()
}

/// One `message.received` per target. `text`, `attachments` and `wake` all come from the row
/// itself when `doc` holds it; otherwise `attachments_unknown` is set and both the body and the
/// wake decision fall back to the announce's own preview.
fn deliver_announced(state: &AppState, ev: &Announced, doc: Option<&ChatDoc>) {
    let chat = &ev.chat;
    let sender = &ev.sender;
    let message_id = ev.message_id.as_str();
    let row = doc.and_then(|d| d.message(message_id));
    let attachments = row.as_ref().map(|row| attachments_of(row, chat));
    // The announce body is a push preview: the sending client cuts it to a fixed length
    // (iPhone 140 chars, Mac 600) for the alert, so it is not the message. The row is.
    let text = row
        .as_ref()
        .map_or_else(|| ev.text.clone(), |row| row.text());
    if attachments.is_none() {
        tracing::warn!(
            chat = %chat.chat_id,
            message = %message_id,
            "announced row not in the relay's doc: message.received sent with attachments unknown"
        );
    }
    let reply_to = reply_json(state, doc, chat, ev.reply_to.as_deref());
    for (p, rule, chat_json) in &ev.targets {
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
        // Asked here rather than at announce time so the question is put to `text` above — the
        // stored row when it arrived. A tag typed past the client's preview cut is in the row
        // and not in the preview, and deciding from the preview dropped the message entirely:
        // the agent was never woken and nobody answered the tester (#2413).
        let wake = match rule {
            WakeRule::Always => true,
            WakeRule::WhenMentioned => mentions_agent(&text, &ev.mentions, p),
        };
        // `body` says which text the decision was put to: the stored row, or the announce
        // preview because the row was not in the doc. Without it a wake has to be inferred
        // from the delivered payload, which is the same field either way.
        tracing::info!(
            chat = %chat.chat_id,
            agent = %p.id,
            handle = %handle_of(p),
            message = %message_id,
            wake,
            rule = match rule {
                WakeRule::Always => "always",
                WakeRule::WhenMentioned => "when_mentioned",
            },
            body = if row.is_some() { "row" } else { "preview" },
            body_chars = text.chars().count(),
            announce_chars = ev.text.chars().count(),
            "outside agent wake decided"
        );
        let mut message = json!({
            "id": message_id,
            "text": text,
            "from": {
                "kind": sender.kind,
                "id": sender.id,
                "name": sender.name,
                "handle": sender.handle,
            },
            "reply_to": reply_to,
            "mentions": ev.mentions,
        });
        match &attachments {
            Some(a) => message["attachments"] = json!(a),
            None => message["attachments_unknown"] = json!(true),
        }
        deliver(
            state,
            &p.id,
            json!({
                "type": "message.received",
                "chat": chat_json,
                "message": message,
                "wake": wake,
            }),
        );
    }
}

/// The blob slot of a chat: the project id for an agent's own chats, `people:<chat>` for DMs and
/// groups — the same slot the apps read `GET /blob/{slot}/{key}` from.
fn slot_of(chat: &ChatRef) -> String {
    match &chat.project_id {
        Some(pid) => pid.clone(),
        None => format!("{}{}", people::PEOPLE_ACL_PREFIX, chat.chat_id),
    }
}

/// `attachments[]` of one row for the agent: every attachment part (pictures, files, GIFs and
/// the PNG fallback of a chart part), `blob` = `<slot>/<key>` for `GET /asp/files/{slot}/{key}`.
pub fn attachments_of(row: &arbos_core::doc::MessageView, chat: &ChatRef) -> Vec<Value> {
    let slot = slot_of(chat);
    let text = row.text();
    row.parts
        .iter()
        .filter(|p| p["kind"] == "attachment")
        .filter_map(|p| {
            let a = p.get("attachment")?;
            let key = a["ref"].as_str().filter(|r| !r.is_empty())?;
            let mime = a["mime"].as_str().unwrap_or("application/octet-stream");
            // The app's tag (image, video, audio, pdf, doc, sheet, slides, file) when it set
            // one, else from the mime; a GIF is always "gif" so a bot can tell it apart.
            let kind = if mime.eq_ignore_ascii_case("image/gif") {
                "gif".to_owned()
            } else {
                a["kind"].as_str().map(str::to_owned).unwrap_or_else(|| {
                    if mime.starts_with("image/") {
                        "image".into()
                    } else if mime.starts_with("video/") {
                        "video".into()
                    } else if mime.starts_with("audio/") {
                        "audio".into()
                    } else {
                        "file".into()
                    }
                })
            };
            Some(json!({
                "blob": format!("{slot}/{key}"),
                "kind": kind,
                "mime": mime,
                "name": a["name"].as_str().unwrap_or(""),
                "size": a["bytes"].as_i64().or_else(|| a["size"].as_i64()),
                "width": a["width"].as_i64(),
                "height": a["height"].as_i64(),
                "caption": if text.trim().is_empty() { Value::Null } else { json!(text) },
                "duration_ms": a["durationMs"].as_i64().or_else(|| a["duration_ms"].as_i64()),
                "chart": a.get("chartPart").is_some(),
                "title": a["title"].as_str(),
                "source": a.get("source").cloned().unwrap_or(Value::Null),
            }))
        })
        .collect()
}

/// How much of the parent's text a `reply_to` carries.
const REPLY_TEXT_MAX: usize = 500;

/// `reply_to` of an event or row: the parent message `{id, from, text (≤ 500 chars),
/// attachments}` so the agent sees what is being answered (Scout, 2026-10-28); `{id}` alone when
/// the parent is not in this chat's doc; `null` for a message that replies to nothing.
pub fn reply_json(
    state: &AppState,
    doc: Option<&arbos_core::doc::ChatDoc>,
    chat: &ChatRef,
    reply_to: Option<&str>,
) -> Value {
    let Some(id) = reply_to.filter(|r| !r.is_empty()) else {
        return Value::Null;
    };
    let Some(parent) = doc.and_then(|d| d.message(id)) else {
        return json!({ "id": id });
    };
    let full = parent.text();
    let text = if full.chars().count() > REPLY_TEXT_MAX {
        let cut: String = full.chars().take(REPLY_TEXT_MAX).collect();
        format!("{cut}…")
    } else {
        full
    };
    json!({
        "id": id,
        "from": from_json(state, &parent, chat),
        "text": text,
        "attachments": attachments_of(&parent, chat),
    })
}

/// One row in the shape `message.received.message` uses (history and events agree); `doc` is
/// the chat it sits in, for the parent of its `reply_to`.
pub fn message_json(
    state: &AppState,
    row: &arbos_core::doc::MessageView,
    chat: &ChatRef,
    doc: &arbos_core::doc::ChatDoc,
) -> Value {
    let mentions: Vec<String> = row.rest["mentions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": row.id,
        "role": row.role,
        "text": row.text(),
        "from": from_json(state, row, chat),
        "reply_to": reply_json(state, Some(doc), chat, row.rest["replyTo"].as_str()),
        "mentions": mentions,
        "attachments": attachments_of(row, chat),
        "created_at": row.rest["createdAt"].as_i64(),
    })
}

/// Who wrote a row, as `message.from` names them.
fn from_json(state: &AppState, row: &arbos_core::doc::MessageView, chat: &ChatRef) -> Value {
    let raw = row.rest["sender"].as_str().unwrap_or("");
    if let Some(uid) = raw.strip_prefix("user:") {
        let s = Sender::user(state, uid);
        json!({ "kind": "user", "id": uid, "name": s.name, "handle": s.handle })
    } else if let Some(pid) = raw.strip_prefix("agent:") {
        let p = state.db.project(pid).ok().flatten();
        json!({ "kind": "agent", "id": pid,
                "name": row.rest["senderName"].as_str().map(str::to_owned).or_else(|| p.as_ref().map(|p| p.name.clone())).unwrap_or_default(),
                "handle": p.as_ref().map(|p| people::agent_slug(&p.name)).unwrap_or_default() })
    } else if row.role == "assistant" {
        // An agent's own chat: the project itself.
        let p = chat
            .project_id
            .as_deref()
            .and_then(|pid| state.db.project(pid).ok().flatten());
        json!({ "kind": "agent", "id": chat.project_id, "name": p.as_ref().map(|p| p.name.clone()).unwrap_or_default(),
                "handle": p.as_ref().map(|p| people::agent_slug(&p.name)).unwrap_or_default() })
    } else if row.role == "user" {
        // An agent's own chat: user rows are the owner's.
        let owner = chat
            .project_id
            .as_deref()
            .and_then(|pid| state.user_of(pid));
        match owner {
            Some(uid) => {
                let s = Sender::user(state, &uid);
                json!({ "kind": "user", "id": uid, "name": s.name, "handle": s.handle })
            }
            None => json!({ "kind": "user" }),
        }
    } else {
        json!({ "kind": "system" })
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

/// `owned` for sibling modules (the session log reader).
pub(crate) fn owned_for(
    state: &AppState,
    principal: &Principal,
    project_id: &str,
) -> Result<(OutsideAgent, Project), (StatusCode, String)> {
    owned(state, principal, project_id)
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
    let log = state.db.asp_log_summary(&project.id).unwrap_or(Value::Null);
    Ok(Json(json!({
        "agent": agent.json(Some(&project)),
        "link": link_for(&state, &project.id),
        "relay_url": state.cfg.public_url,
        "log": log,
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

/// Absent ≠ null: `Some(None)` clears, `None` leaves the field alone.
fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(d).map(Some)
}

/// `PATCH /outside-agents/{id}`: optional fields, each applied only when present; `null` clears.
#[derive(Deserialize, Default)]
pub struct PatchBody {
    /// Scope name → on/off; a partial object merges into the owner's overrides.
    #[serde(default)]
    pub scopes: Option<serde_json::Map<String, Value>>,
    /// Wake URL (`https://…`); `null` clears it and forgets the last delivery.
    #[serde(default, deserialize_with = "double_option")]
    pub wake_url: Option<Option<String>>,
    /// The `Authorization` header value the Wake URL expects, verbatim; `null` clears.
    #[serde(default, deserialize_with = "double_option")]
    pub wake_auth: Option<Option<String>>,
}

/// `PATCH /outside-agents/{id} {scopes?, wake_url?, wake_auth?}` — the owner's switches (the
/// relay admin may set them too) and the Wake URL; answers the agent as `GET` does. The header
/// value is stored and never returned; the URL is shown back.
pub async fn patch(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
    Json(b): Json<PatchBody>,
) -> ApiResult {
    let (agent, project) = match &principal {
        Principal::Admin => {
            let project = state
                .db
                .project(&id)
                .map_err(internal)?
                .ok_or((StatusCode::NOT_FOUND, "no such agent".to_owned()))?;
            let agent = state
                .db
                .outside_agent(&id)
                .map_err(internal)?
                .ok_or((StatusCode::NOT_FOUND, "no such connected agent".to_owned()))?;
            (agent, project)
        }
        _ => owned(&state, &principal, &id)?,
    };
    if let Some(scopes) = &b.scopes {
        if let Some(bad) = scopes.keys().find(|k| !SCOPES.iter().any(|(n, _)| n == *k)) {
            return Err((StatusCode::BAD_REQUEST, format!("unknown scope {bad}")));
        }
        state
            .db
            .set_outside_scopes(&project.id, scopes)
            .map_err(internal)?;
        let changed = Value::Object(scopes.clone()).to_string();
        tracing::info!(project = %project.id, scopes = %changed, "outside agent scopes changed");
    }
    if (b.wake_url.is_some() || b.wake_auth.is_some()) && agent.revoked_at.is_some() {
        return Err((StatusCode::CONFLICT, "access is revoked".into()));
    }
    let url = match b.wake_url {
        Some(Some(raw)) if !raw.trim().is_empty() => Some(Some(
            outside_wake::check_url(&raw).map_err(|e| (StatusCode::BAD_REQUEST, e.to_owned()))?,
        )),
        Some(_) => Some(None),
        None => None,
    };
    let auth = match b.wake_auth {
        Some(Some(raw)) if !raw.trim().is_empty() => Some(Some(
            outside_wake::check_auth(&raw).map_err(|e| (StatusCode::BAD_REQUEST, e.to_owned()))?,
        )),
        Some(_) => Some(None),
        None => None,
    };
    if let Some(url) = &url {
        if agent.wake_url != *url {
            state
                .db
                .set_wake_url(&project.id, url.as_deref())
                .map_err(internal)?;
        }
    }
    if let Some(auth) = &auth {
        state
            .db
            .set_wake_auth(&project.id, auth.as_deref())
            .map_err(internal)?;
    }
    if url.is_some() || auth.is_some() {
        tracing::info!(
            project = %project.id,
            url_set = url.as_ref().map(|u| u.is_some()),
            auth_set = auth.as_ref().map(|a| a.is_some()),
            "outside agent wake settings changed"
        );
    }
    let agent = state
        .db
        .outside_agent(&project.id)
        .map_err(internal)?
        .ok_or_else(|| internal("row vanished"))?;
    Ok(Json(json!({
        "agent": agent.json(Some(&project)),
        "link": link_for(&state, &project.id),
        "relay_url": state.cfg.public_url,
    }))
    .into_response())
}

fn wake_result_json(last: &LastWake) -> Value {
    json!({
        "ok": last.error.is_none(),
        "status": last.status,
        "error": last.error,
        "at": last.at,
    })
}

/// `POST /outside-agents/{id}/wake/test` (owner; the Connection card's "Send test wake"): one
/// `wake.test` nudge now, no debounce, no retry → `{ok, status, error, at}`.
pub async fn wake_test(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<String>,
) -> ApiResult {
    let (_, project) = owned(&state, &principal, &id)?;
    let last = outside_wake::send_now(&state, &project.id, &Nudge::test(&project.id))
        .await
        .map_err(|e| (StatusCode::CONFLICT, e))?;
    Ok(Json(wake_result_json(&last)).into_response())
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

pub(crate) fn chat_json_for(state: &AppState, project: &Project, chat_id: &str) -> Option<Value> {
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
            "scopes": Value::Object(me.agent.effective_scopes()),
        },
        "chats": chats,
        "inbox_head": state.db.asp_events_head(&me.project.id).unwrap_or(0),
        "wake_url_set": me.agent.wake_url.is_some() && me.agent.wake_auth_set,
        "wake_header": outside_wake::HEADER,
        "last_wake": me.agent.wake_last.as_ref().map(LastWake::json),
    }))
    .into_response()
}

/// `POST /asp/wake/test` (agent key) — the same test nudge the Connection card's button sends.
pub async fn asp_wake_test(State(state): State<AppState>, me: AgentPrincipal) -> Response {
    match outside_wake::send_now(&state, &me.project.id, &Nudge::test(&me.project.id)).await {
        Ok(last) => Json(wake_result_json(&last)).into_response(),
        Err(e) => api_error(StatusCode::CONFLICT, "no_wake_url", &e),
    }
}

#[derive(Deserialize)]
pub struct InboxQuery {
    #[serde(default)]
    pub since: i64,
    #[serde(default)]
    pub wait: u64,
    #[serde(default)]
    pub limit: Option<i64>,
    /// Rows of context to attach to each wake event (0 = none; ≤ 20; `chat.read`).
    #[serde(default)]
    pub context: usize,
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
    if !me.agent.allows("chat.read") {
        return api_error(
            StatusCode::FORBIDDEN,
            "scope",
            "reading is switched off for this agent",
        );
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let wait = q.wait.min(WAIT_MAX_S);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(wait);
    let n = waiter(&me.project.id);
    loop {
        // Register interest *before* reading the table: `notify_waiters` wakes only waiters that
        // already exist, so an event stored between the read and the wait would otherwise park
        // this call until `wait` runs out (up to 25 s) instead of answering at once.
        let notified = n.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let mut events = state
            .db
            .asp_events_after(&me.project.id, q.since, limit)
            .unwrap_or_default();
        if !events.is_empty() || wait == 0 {
            crate::outside_sidecar::attach_context(&state, &me.project, &mut events, q.context);
            return Json(events_json(events, q.since)).into_response();
        }
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

pub(crate) type EventResult = Result<Value, (&'static str, String)>;

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

/// Why a `message.delta` / `message.end` was refused before anything was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamRefused {
    NoSuchMessage,
    NotYours,
    Finished,
    TooLarge,
}

impl StreamRefused {
    fn event_error(self) -> (&'static str, String) {
        let (code, msg) = match self {
            StreamRefused::NoSuchMessage => ("not_found", "no such message in this chat"),
            StreamRefused::NotYours => ("forbidden", "this agent did not start that message"),
            StreamRefused::Finished => ("closed", "that message is already finished"),
            StreamRefused::TooLarge => ("too_large", "message over 64 KB in total"),
        };
        (code, msg.to_owned())
    }
}

impl std::fmt::Display for StreamRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.event_error().1)
    }
}

impl std::error::Error for StreamRefused {}

pub(crate) async fn apply_event(state: &AppState, me: &AgentPrincipal, e: &Value) -> EventResult {
    let ty = field(e, "type").ok_or(("bad_request", "type required".to_owned()))?;
    let pid = me.project.id.clone();
    match ty {
        "presence.heartbeat" => {
            let _ = state.db.touch_outside_agent(&pid, e.get("agent"));
            Ok(json!({ "ok": true, "type": ty }))
        }
        "typing" | "status" => {
            if !me.agent.allows("presence") {
                return Err((
                    "scope",
                    "typing and status are switched off for this agent".to_owned(),
                ));
            }
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
            if !me.agent.allows("chat.post") {
                return Err(("scope", "posting is switched off for this agent".to_owned()));
            }
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
            if !me.agent.allows("chat.post") {
                return Err(("scope", "posting is switched off for this agent".to_owned()));
            }
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
            let blank = Arc::new(AtomicBool::new(false));
            let blank2 = blank.clone();
            let device = device_of(&pid);
            let device_in = device.clone();
            let written = crate::notices::write_doc_as(
                state,
                project_id.as_deref(),
                chat_id,
                &device,
                move |doc| {
                    // Only a row this agent started and has not finished takes more text.
                    let row = doc.message(&m).ok_or(StreamRefused::NoSuchMessage)?;
                    if row.rest["deviceId"].as_str() != Some(device_in.as_str()) {
                        return Err(StreamRefused::NotYours.into());
                    }
                    if row.status != MessageStatus::Streaming.as_str() {
                        return Err(StreamRefused::Finished.into());
                    }
                    let total = match (ending, t.as_deref()) {
                        (true, Some(whole)) => whole.len(),
                        (_, Some(delta)) => row.text().len() + delta.len(),
                        (_, None) => 0,
                    };
                    if total > MESSAGE_MAX {
                        return Err(StreamRefused::TooLarge.into());
                    }
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
                        if !is_people {
                            settle_send(doc, r.as_deref(), CommandStatus::Done);
                        }
                        // A stream that ends with no words and no attachment is not a message:
                        // the row goes rather than stay as a blank line.
                        if doc
                            .message(&m)
                            .is_some_and(|row| !row.has_visible_content())
                        {
                            doc.delete_message(&m)?;
                            blank2.store(true, Ordering::Relaxed);
                            return Ok(());
                        }
                        doc.finish_message(&m, MessageStatus::Done);
                        if let Some(row) = doc.message(&m) {
                            *full2.lock().unwrap() = row.text();
                        }
                    }
                    Ok(())
                },
            )
            .await;
            if let Err(e) = written {
                let Some(refused) = e.downcast_ref::<StreamRefused>() else {
                    return Err(("write_failed", e.to_string()));
                };
                if matches!(
                    refused,
                    StreamRefused::NoSuchMessage | StreamRefused::Finished
                ) {
                    let mut open = open_streams().lock().unwrap();
                    if open.get(&key).is_some_and(|(id, _)| *id == msg_id) {
                        open.remove(&key);
                    }
                }
                return Err(refused.event_error());
            }
            if ending {
                open_streams().lock().unwrap().remove(&key);
                state.activity.lock().await.remove(&pid);
                if blank.load(Ordering::Relaxed) {
                    return Err((
                        "bad_request",
                        "text required: the message ended empty and was not posted".to_owned(),
                    ));
                }
                let whole = full.lock().unwrap().clone();
                announce(state, &me.project, &chat, &msg_id, &whole).await;
            }
            Ok(json!({ "ok": true, "type": ty, "message": msg_id, "chat": chat_id }))
        }
        "message.react" => {
            if !me.agent.allows("chat.react") {
                return Err((
                    "scope",
                    "reacting is switched off for this agent".to_owned(),
                ));
            }
            let chat_id = field(e, "chat").ok_or(("bad_request", "chat required".to_owned()))?;
            let chat = chat_json_for(state, &me.project, chat_id)
                .ok_or(("forbidden", "not a chat of this agent".to_owned()))?;
            let msg_id = field(e, "message")
                .ok_or(("bad_request", "message required".to_owned()))?
                .to_owned();
            let emoji = field(e, "emoji")
                .ok_or(("bad_request", "emoji required".to_owned()))?
                .to_owned();
            if emoji.chars().count() > 16 {
                return Err(("bad_request", "emoji too long".to_owned()));
            }
            let on = e.get("on").and_then(|v| v.as_bool()).unwrap_or(true);
            let project_id = chat["project_id"].as_str().map(str::to_owned);
            let sender = format!("agent:{pid}");
            let (m, em) = (msg_id.clone(), emoji.clone());
            crate::notices::write_doc_as(
                state,
                project_id.as_deref(),
                chat_id,
                &device_of(&pid),
                move |doc| {
                    if doc.message(&m).is_none() {
                        anyhow::bail!("no such message");
                    }
                    doc.set_reaction(&m, &em, &sender, on)?;
                    Ok(())
                },
            )
            .await
            .map_err(|e| ("write_failed", e.to_string()))?;
            let _ = state.db.touch_outside_agent(&pid, None);
            Ok(json!({ "ok": true, "type": ty, "message": msg_id, "chat": chat_id, "on": on }))
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
    if !me.agent.allows("chat.read") {
        return api_error(
            StatusCode::FORBIDDEN,
            "scope",
            "reading is switched off for this agent",
        );
    }
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| run_socket(socket, state, me, q.since, q.context))
}

async fn run_socket(
    socket: WebSocket,
    state: AppState,
    me: AgentPrincipal,
    mut cursor: i64,
    context: usize,
) {
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
    let n = waiter(&pid);
    loop {
        // Same ordering as the inbox: interest first, then the read, then the wait.
        let notified = n.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // Drain what is queued first.
        let mut events = state
            .db
            .asp_events_after(&pid, cursor, 100)
            .unwrap_or_default();
        crate::outside_sidecar::attach_context(&state, &me.project, &mut events, context);
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

/// May this agent read blobs in `slot`? Its own project's slot, or `people:<chat>` for a people
/// chat it sits in.
fn slot_allowed(state: &AppState, project: &Project, slot: &str) -> bool {
    if slot == project.id {
        return true;
    }
    match slot.strip_prefix(people::PEOPLE_ACL_PREFIX) {
        Some(chat_id) => chat_json_for(state, project, chat_id).is_some(),
        None => false,
    }
}

/// `GET /asp/files/{slot}/{key}` — the bytes of a picture / file / GIF / chart PNG named in an
/// event's `attachments[].blob`, for agents with `files.read`; only blobs of the agent's chats.
pub async fn file(
    State(state): State<AppState>,
    me: AgentPrincipal,
    Path((slot, key)): Path<(String, String)>,
    Query(size): Query<crate::avatar_image::SizeQuery>,
) -> Response {
    if !me.agent.allows("files.read") {
        return api_error(StatusCode::FORBIDDEN, "scope", "files are switched off for this agent — the owner can turn on \"Open files shared in chat\"");
    }
    if !slot_allowed(&state, &me.project, &slot) {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "not a file of this agent's chats",
        );
    }
    // `?w=<px>`: a picture scaled down on the way out (the apps' thumbnail path); anything
    // else, or no `w`, is the original bytes with the stored mime.
    if let Some(w) = size.w {
        if let crate::blobs::StoredImage::Image(kind) =
            crate::blobs::stored_image(&state, &slot, &key).await
        {
            if let Some(bytes) = crate::blobs::read(&state, &slot, &key).await {
                return crate::avatar_image::respond(
                    &state,
                    crate::avatar_image::Master { kind, bytes },
                    Some(w),
                    None,
                )
                .await;
            }
        }
    }
    crate::blobs::serve(&state, &slot, &key).await
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    /// A message id: rows strictly before it (default: the newest rows).
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// `GET /asp/chats/{id}/messages?before=<message id>&limit=<n ≤ 100>` — history of one of the
/// agent's chats, oldest first, each row in the `message.received.message` shape (`chat.read`).
pub async fn history(
    State(state): State<AppState>,
    me: AgentPrincipal,
    Path(chat_id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Response {
    if !me.agent.allows("chat.read") {
        return api_error(
            StatusCode::FORBIDDEN,
            "scope",
            "reading is switched off for this agent",
        );
    }
    let Some(chat) = chat_json_for(&state, &me.project, &chat_id) else {
        return api_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "not a chat of this agent",
        );
    };
    let chat_ref = match chat["project_id"].as_str() {
        Some(pid) => ChatRef::project(pid, &chat_id),
        None => ChatRef::people(&chat_id),
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let doc = match crate::matrix::backfill::load_doc(
        &state,
        &chat_id,
        chat_ref.project_id.as_deref(),
    ) {
        Ok(Some(doc)) => doc,
        Ok(None) => return Json(
            json!({ "chat": chat, "messages": [], "has_more": false, "next_before": Value::Null }),
        )
        .into_response(),
        Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, "db", e.to_string()),
    };
    // A row with no words and no visible attachment (an agent-to-agent hand-off row, a job's
    // host row, an empty stream) reads as a blank message with no sender to the agent.
    let rows = doc.messages();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|m| m.role == "user" || m.role == "assistant" || m.role == "project")
        .filter(|m| m.has_visible_content())
        .collect();
    let end = match &q.before {
        Some(b) => rows.iter().position(|m| &m.id == b).unwrap_or(rows.len()),
        None => rows.len(),
    };
    let start = end.saturating_sub(limit);
    let messages: Vec<Value> = rows[start..end]
        .iter()
        .map(|m| message_json(&state, m, &chat_ref, &doc))
        .collect();
    Json(json!({
        "chat": chat,
        "messages": messages,
        "has_more": start > 0,
        "next_before": rows.get(start).map(|m| m.id.clone()).filter(|_| start > 0),
    }))
    .into_response()
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
         Agent instructions (paste into your agent): {relay}/llms.txt\n\
         Sidecar installer:                           {relay}/sidecar/install.sh\n\n\
         Relay:   {relay}\n\
         Agent:   {id}\n\n\
         Every call carries the key the Arbos app showed you once:  Authorization: Bearer arbk_…\n\n\
         Read:    GET  {relay}/asp/inbox?since=0&wait=25      (events; `next` is the next `since`)\n\
         Live:    WS   {relay}/asp/socket?since=0              (same events, pushed)\n\
         History: GET  {relay}/asp/chats/<chat id>/messages?before=<message id>&limit=50   (same row shape, oldest first)\n\
         Reply:   message.reply_to is the parent message {{id, from, text (first 500 chars), attachments}} when the message answers one\n\
         Files:   GET  {relay}/asp/files/<blob>   where <blob> is attachments[].blob on a message (any kind: pictures, GIFs, video, voice memos, files; add ?w=<px> for a smaller picture);\n\
                  needs the owner's switch \"Open files shared in chat\" (files.read) on the agent's profile\n\
         Write:   POST {relay}/asp/events  {{\"type\":\"message.post\",\"chat\":\"<chat id from the event>\",\"text\":\"…\",\"reply_to\":\"<message id>\"}}\n\
         Stream:  message.start → message.delta (text) → message.end\n\
         Typing:  {{\"type\":\"typing\",\"chat\":\"…\",\"on\":true}}   Status: {{\"type\":\"status\",\"chat\":\"…\",\"text\":\"reading Stripe\"}}\n\
         Alive:   {{\"type\":\"presence.heartbeat\"}} every 60 s (any call renews the lease)\n\n\
         Wake URL (Grok Bot routine with a webhook trigger, or any bot with an HTTPS endpoint):\n\
         in Arbos open the agent's profile → Connection → Wake URL, paste the webhook URL and the\n\
         exact Authorization header value the routine panel shows, Save, then Send test wake.\n\
         When a message needs an answer the relay POSTs there at once (no message text):\n\
         {{\"agent\":\"{id}\",\"chat\":\"<chat id>\",\"event_id\":N,\"type\":\"message.received\"}}\n\
         with headers  Authorization: <your value>  and  X-Arbos-Signature: sha256=<hex HMAC-SHA256 of the body, key = that value>.\n\
         On a nudge: GET {relay}/asp/inbox?since=<last next>  and answer the events with wake=true.\n\
         At most one nudge per 5 s per agent; keep a slow poll (every 5 min) as the fallback.\n\
         Test from the agent side: POST {relay}/asp/wake/test (same key) → {{ok,status,error}}.\n\n\
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
            let _ = state.db.prune_asp_log();
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
        // A collective names every agent in the chat and wakes none of them on its own; the
        // agent's own handle in the same sentence still does.
        assert!(!mentions_agent("@here standup", &["agent:p1".into()], &p));
        assert!(!mentions_agent(
            "@agents take this",
            &["agent:p1".into()],
            &p
        ));
        assert!(mentions_agent(
            &format!("@here and @{handle}"),
            &["agent:p1".into()],
            &p
        ));
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

    /// A picture on a row reaches the agent as `attachments[]` with a fetchable blob id; files
    /// are off by default, the owner's PATCH turns them on; history rows share the shape.
    #[tokio::test]
    async fn attachments_scopes_and_history() {
        use crate::people::tests_support::{group, person, project_of};
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;
        use arbos_core::doc::{UserAttachment, UserMessageOpts};

        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 7101);
        let (other, _) = person(&state, 7102);
        let pid = project_of(&state, &owner, "Grok Bot");
        state
            .db
            .insert_outside_agent(&pid, &owner, "grok", &hash(&mint_key()))
            .unwrap();
        let g = group(&state, &[&owner, &other]);
        state
            .db
            .add_member(&g, &format!("agent:{pid}"), "agent", &owner, &json!({}))
            .unwrap();
        // A person posts a picture with a caption, as the apps write it.
        let msg_id = ulid();
        let m2 = msg_id.clone();
        let o2 = other.clone();
        crate::notices::write_doc(&state, None, &g, move |doc| {
            doc.append_user_message(
                &m2,
                "the chart from the call",
                "dev:test",
                &UserMessageOpts {
                    attachments: vec![UserAttachment {
                        name: "chart.png".into(),
                        mime: "image/png".into(),
                        kind: Some("image".into()),
                        blob_ref: Some("abc123".into()),
                        url: None,
                        source: None,
                    }],
                    sender: Some(format!("user:{o2}")),
                    ..Default::default()
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let me = Sender::user(&state, &other);
        on_announce(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            "the chart from the call",
            None,
            &[],
        );
        let events = state.db.asp_events_after(&pid, 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        let att = &events[0].1["message"]["attachments"];
        assert_eq!(att.as_array().map(|a| a.len()), Some(1));
        assert_eq!(att[0]["blob"], format!("people:{g}/abc123"));
        assert_eq!(att[0]["kind"], "image");
        assert_eq!(att[0]["mime"], "image/png");
        assert_eq!(att[0]["name"], "chart.png");
        assert_eq!(att[0]["caption"], "the chart from the call");

        // Every attachment kind goes through: a GIF the app tagged "image", a voice memo, a PDF.
        let mid = ulid();
        let (m3, o4) = (mid.clone(), other.clone());
        crate::notices::write_doc(&state, None, &g, move |doc| {
            doc.append_user_message(
                &m3,
                "",
                "dev:test",
                &UserMessageOpts {
                    attachments: vec![
                        UserAttachment {
                            name: "nope.gif".into(),
                            mime: "image/gif".into(),
                            kind: Some("image".into()),
                            blob_ref: Some("g1".into()),
                            url: None,
                            source: None,
                        },
                        UserAttachment {
                            name: "memo.m4a".into(),
                            mime: "audio/mp4".into(),
                            kind: Some("audio".into()),
                            blob_ref: Some("a1".into()),
                            url: None,
                            source: None,
                        },
                        UserAttachment {
                            name: "deck.pdf".into(),
                            mime: "application/pdf".into(),
                            kind: None,
                            blob_ref: Some("p1".into()),
                            url: None,
                            source: None,
                        },
                        UserAttachment {
                            name: "clip.mp4".into(),
                            mime: "video/mp4".into(),
                            kind: Some("video".into()),
                            blob_ref: Some("v1".into()),
                            url: None,
                            source: None,
                        },
                    ],
                    sender: Some(format!("user:{o4}")),
                    ..Default::default()
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let doc = crate::matrix::backfill::load_doc(&state, &g, None)
            .unwrap()
            .unwrap();
        let row = doc.message(&mid).unwrap();
        let kinds: Vec<String> = attachments_of(&row, &ChatRef::people(&g))
            .iter()
            .map(|a| a["kind"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(kinds, ["gif", "audio", "file", "video"]);
        let all = attachments_of(&row, &ChatRef::people(&g));
        assert_eq!(all[2]["mime"], "application/pdf");
        assert_eq!(all[0]["caption"], Value::Null);

        // Scopes: files off by default, PATCH merges, unknown names are ignored by the db layer.
        let agent = state.db.outside_agent(&pid).unwrap().unwrap();
        assert!(!agent.allows("files.read"));
        assert!(agent.allows("chat.read") && agent.allows("chat.post"));
        let mut patch = serde_json::Map::new();
        patch.insert("files.read".into(), json!(true));
        state.db.set_outside_scopes(&pid, &patch).unwrap();
        let agent = state.db.outside_agent(&pid).unwrap().unwrap();
        assert!(agent.allows("files.read"));
        assert_eq!(agent.effective_scopes().len(), SCOPES.len());
        assert_eq!(agent.json(None)["scopes"]["files.read"], true);
        assert_eq!(agent.json(None)["scopes"]["secret.use"], false);
        assert!(slot_allowed(
            &state,
            &state.db.project(&pid).unwrap().unwrap(),
            &format!("people:{g}")
        ));
        assert!(!slot_allowed(
            &state,
            &state.db.project(&pid).unwrap().unwrap(),
            "people:nope"
        ));

        // History rows: same shape as the event's `message`.
        let doc = crate::matrix::backfill::load_doc(&state, &g, None)
            .unwrap()
            .unwrap();
        let rows = doc.messages();
        let chat_ref = ChatRef::people(&g);
        let j = message_json(&state, &rows[0], &chat_ref, &doc);
        assert_eq!(j["id"], msg_id);
        assert_eq!(j["from"]["kind"], "user");
        assert_eq!(j["from"]["id"], other);
        assert_eq!(j["attachments"][0]["blob"], format!("people:{g}/abc123"));
        assert_eq!(j["text"], "the chart from the call");
        assert_eq!(j["reply_to"], Value::Null);

        // A reply: the event's `reply_to` carries the parent — its sender, text and picture.
        let reply_id = ulid();
        let (r2, parent, o3) = (reply_id.clone(), msg_id.clone(), owner.clone());
        crate::notices::write_doc(&state, None, &g, move |doc| {
            doc.append_user_message(
                &r2,
                "is this the final one?",
                "dev:test",
                &UserMessageOpts {
                    sender: Some(format!("user:{o3}")),
                    ..Default::default()
                },
            )?;
            doc.set_message_fields(&r2, &[("replyTo", json!(parent))]);
            Ok(())
        })
        .await
        .unwrap();
        let me = Sender::user(&state, &owner);
        on_announce(
            &state,
            &ChatRef::people(&g),
            &me,
            &reply_id,
            "is this the final one?",
            Some(&msg_id),
            &[],
        );
        let events = state.db.asp_events_after(&pid, 1, 10).unwrap();
        let r = &events[0].1["message"]["reply_to"];
        assert_eq!(r["id"], msg_id);
        assert_eq!(r["from"]["id"], other);
        assert_eq!(r["text"], "the chart from the call");
        assert_eq!(r["attachments"][0]["blob"], format!("people:{g}/abc123"));
        // An unknown parent keeps the id alone.
        assert_eq!(
            reply_json(&state, Some(&doc), &chat_ref, Some("nope")),
            json!({ "id": "nope" })
        );
    }

    /// A group with one outside agent in it: (state, agent project id, group id, a member).
    fn outside_group(seed: i64) -> (AppState, String, String, String) {
        use crate::people::tests_support::{group, person, project_of};
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;

        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, seed);
        let (other, _) = person(&state, seed + 1);
        let pid = project_of(&state, &owner, "Grok Bot");
        state
            .db
            .insert_outside_agent(&pid, &owner, "grok", &hash(&mint_key()))
            .unwrap();
        let g = group(&state, &[&owner, &other]);
        state
            .db
            .add_member(&g, &format!("agent:{pid}"), "agent", &owner, &json!({}))
            .unwrap();
        (state, pid, g, other)
    }

    fn picture_opts(sender: &str) -> arbos_core::doc::UserMessageOpts {
        arbos_core::doc::UserMessageOpts {
            attachments: vec![arbos_core::doc::UserAttachment {
                name: "shot.png".into(),
                mime: "image/png".into(),
                kind: None,
                blob_ref: Some("att-shot".into()),
                url: None,
                source: None,
            }],
            sender: Some(format!("user:{sender}")),
            ..Default::default()
        }
    }

    async fn events_eventually(state: &AppState, pid: &str) -> Vec<(i64, Value)> {
        for _ in 0..100 {
            let events = state.db.asp_events_after(pid, 0, 10).unwrap();
            if !events.is_empty() {
                return events;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        Vec::new()
    }

    /// #2305: the app's announce reached the relay 440 ms before its row (Arbonites
    /// 01M64SV6F5…), and the event said `attachments: []` for a message with a screenshot. The
    /// event now waits for the row and lists the picture.
    #[tokio::test]
    async fn announce_before_its_row_still_lists_the_picture() {
        let (state, pid, g, other) = outside_group(7201);
        let msg_id = ulid();
        let me = Sender::user(&state, &other);
        on_announce_waiting(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            "look at this",
            None,
            &[],
            std::time::Duration::from_secs(3),
        );
        assert!(
            state.db.asp_events_after(&pid, 0, 10).unwrap().is_empty(),
            "no event before the row is in"
        );
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let (m, o) = (msg_id.clone(), other.clone());
        crate::notices::write_doc(&state, None, &g, move |doc| {
            doc.append_user_message(&m, "look at this", "dev:test", &picture_opts(&o))?;
            Ok(())
        })
        .await
        .unwrap();
        let events = events_eventually(&state, &pid).await;
        assert_eq!(events.len(), 1);
        let msg = &events[0].1["message"];
        assert_eq!(msg["id"], msg_id);
        assert_eq!(
            msg["attachments"][0]["blob"],
            format!("people:{g}/att-shot")
        );
        assert_eq!(msg["attachments"][0]["kind"], "image");
        assert_eq!(msg["attachments"][0]["mime"], "image/png");
        assert!(msg.get("attachments_unknown").is_none());
    }

    /// The announce body is the push preview the sending client cut to a fixed length
    /// (`RelayAPI.swift` takes 140 chars, `arbos-mac` 600). Delivering it as the message gave
    /// outside agents a body cut mid-word with no marker: Arbonites 01M5MHDEZF… asp_event 845
    /// ended "…to a view with the result messa" and the tester had to re-send the rest by hand.
    #[tokio::test]
    async fn announce_delivers_the_row_not_the_push_preview() {
        let full = "@grok on mobile when you press search and type in keyword and tap on \
                    result message sometimes it scrolls you to a view with the result message \
                    in view but sometimes it takes you to an area where the result message is \
                    not in view but nearby";
        let preview: String = full.chars().take(140).collect();
        assert!(full.len() > preview.len(), "the preview is the shorter one");

        let (state, pid, g, other) = outside_group(7221);
        let msg_id = ulid();
        let (m, o, f) = (msg_id.clone(), other.clone(), full.to_owned());
        crate::notices::write_doc(&state, None, &g, move |doc| {
            doc.append_user_message(
                &m,
                &f,
                "dev:phone",
                &arbos_core::doc::UserMessageOpts {
                    sender: Some(format!("user:{o}")),
                    ..Default::default()
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let me = Sender::user(&state, &other);
        on_announce_waiting(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            &preview,
            None,
            &[],
            std::time::Duration::from_secs(1),
        );
        let events = events_eventually(&state, &pid).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1["message"]["text"], full);
    }

    /// #2413: the tag itself can fall past the client's preview cut. Arbonites 01M66NSXQW…
    /// (asp_event 732) stored 142 characters ending "can you investigate the @grok"; the iPhone's
    /// 140-character preview left "the @gr", so deciding `wake` from the preview sent the event
    /// with `wake: false`. The agent was never woken, the attached screenshot was never opened
    /// and nobody answered the tester. The row carries the tag, so the row is what decides.
    #[tokio::test]
    async fn a_mention_past_the_preview_cut_still_wakes_the_agent() {
        let full = "On iOS often times when people send messages while im typing the chat just \
                    goes black like in the attached image can you investigate the @grok-bot";
        let preview: String = full.chars().take(140).collect();
        assert!(
            !preview.contains("@grok-bot"),
            "the preview must not carry the tag, or the test proves nothing: {preview:?}"
        );

        let (state, pid, g, other) = outside_group(7231);
        let msg_id = ulid();
        let (m, o, f) = (msg_id.clone(), other.clone(), full.to_owned());
        crate::notices::write_doc(&state, None, &g, move |doc| {
            doc.append_user_message(
                &m,
                &f,
                "dev:phone",
                &arbos_core::doc::UserMessageOpts {
                    sender: Some(format!("user:{o}")),
                    ..Default::default()
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let me = Sender::user(&state, &other);
        on_announce_waiting(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            &preview,
            None,
            // The iPhone sends no `mentions`, so the text is all there is to go on.
            &[],
            std::time::Duration::from_secs(1),
        );
        let events = events_eventually(&state, &pid).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1["message"]["text"], full);
        assert_eq!(
            events[0].1["wake"], true,
            "the tag is in the row, so the agent wakes: {}",
            events[0].1
        );
    }

    /// Every app announces a message with `mentions` left out — it resolves them into the row it
    /// writes and posts the id and the text alone — so the documented `mentions` on
    /// `message.received` was empty for every message a person ever sent from an app, while a
    /// refetch of the same row a second later listed them. The relay resolves them on the
    /// announce now, from the roster it already has.
    ///
    /// `@here` is everyone in the chat including its agents (Jacob, 2026-10-24), so the event
    /// names this agent — and deliberately does not wake it. `wake` is the only signal an
    /// outside agent gets and it reads as "answer this", so one `@here` in a room with three
    /// agents would be three answers. The array is corrected here; the wake waits for the
    /// protocol question (a turn and a verdict as separate fields) to be answered.
    #[tokio::test]
    async fn an_app_announce_resolves_the_mentions_it_left_out() {
        let (state, pid, g, other) = outside_group(7281);
        let msg_id = ulid();
        let me = Sender::user(&state, &other);
        crate::notify::announce_with(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            "@here standup in five",
            None,
            // What the announce carries today: nothing.
            &[],
        )
        .await;

        let events = events_eventually(&state, &pid).await;
        assert_eq!(events.len(), 1);
        let mentions = events[0].1["message"]["mentions"].clone();
        let owner = state.db.project(&pid).unwrap().unwrap().user_id;
        let named: Vec<&str> = mentions
            .as_array()
            .expect("mentions")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(named.len(), 2, "{mentions}");
        assert!(named.contains(&format!("user:{owner}").as_str()));
        assert!(named.contains(&format!("agent:{pid}").as_str()));
        assert!(
            !named.contains(&format!("user:{other}").as_str()),
            "the writer is not mentioned by their own @here"
        );
        assert_eq!(
            events[0].1["wake"], false,
            "a collective names the agent and leaves the wake as it is: {}",
            events[0].1
        );
    }

    /// The carve-out above must not swallow a real tag: `@here` alongside the agent's own handle
    /// wakes it, as the handle alone always has.
    #[tokio::test]
    async fn a_tag_still_wakes_when_a_collective_names_everyone() {
        let (state, pid, g, other) = outside_group(7282);
        let handle = handle_of(&state.db.project(&pid).unwrap().unwrap());
        assert!(!handle.is_empty());
        let msg_id = ulid();
        let me = Sender::user(&state, &other);
        crate::notify::announce_with(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            &format!("@here and @{handle} please take this one"),
            None,
            &[],
        )
        .await;

        let events = events_eventually(&state, &pid).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1["wake"], true, "{}", events[0].1);
    }

    /// The other half of #2413: a group message that tags nobody must still not wake the agent.
    /// Deciding from the row must not turn the wake rule into "always".
    #[tokio::test]
    async fn a_long_group_message_that_tags_nobody_does_not_wake_the_agent() {
        let full = "I do also see the appeal of swipe to reply but having to swipe on a specific \
                    area that is not another persons chat to swipe out of the chat is too \
                    arbitrary and it keeps catching me out";
        let preview: String = full.chars().take(140).collect();
        assert!(full.chars().count() > 140, "the body outruns the preview");

        let (state, pid, g, other) = outside_group(7241);
        let msg_id = ulid();
        let (m, o, f) = (msg_id.clone(), other.clone(), full.to_owned());
        crate::notices::write_doc(&state, None, &g, move |doc| {
            doc.append_user_message(
                &m,
                &f,
                "dev:phone",
                &arbos_core::doc::UserMessageOpts {
                    sender: Some(format!("user:{o}")),
                    ..Default::default()
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let me = Sender::user(&state, &other);
        on_announce_waiting(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            &preview,
            None,
            &[],
            std::time::Duration::from_secs(1),
        );
        let events = events_eventually(&state, &pid).await;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1["message"]["text"], full);
        assert_eq!(
            events[0].1["wake"], false,
            "nobody was tagged: {}",
            events[0].1
        );
    }

    /// When the row never arrives the body falls back to the preview, and so must the wake
    /// decision — a missing row must not silently become "no". Here the preview carries the tag,
    /// so the agent wakes even though there is no row to confirm it from.
    #[tokio::test]
    async fn with_no_row_the_wake_decision_falls_back_to_the_preview() {
        let (state, pid, g, other) = outside_group(7251);
        let me = Sender::user(&state, &other);
        let msg_id = ulid();
        on_announce_waiting(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            "@grok-bot are you there?",
            None,
            &[],
            std::time::Duration::ZERO,
        );
        let events = state.db.asp_events_after(&pid, 0, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].1["message"]["attachments_unknown"], true,
            "no row: {}",
            events[0].1
        );
        assert_eq!(
            events[0].1["wake"], true,
            "the preview carries the tag: {}",
            events[0].1
        );
    }

    /// #2305, divergent chat: the row is stored but depends on history the relay lost, so the
    /// rebuilt doc cannot show it. The event must not claim `attachments: []`.
    #[tokio::test]
    async fn row_the_doc_cannot_apply_is_attachments_unknown() {
        let (state, pid, g, other) = outside_group(7211);
        crate::notices::write_doc(&state, None, &g, |doc| {
            doc.append_user_message(&ulid(), "hello", "dev:test", &Default::default())?;
            Ok(())
        })
        .await
        .unwrap();
        // A device holds a message the relay never got, then writes a picture on top of it.
        let device = ChatDoc::new(None, false);
        device
            .append_user_message(&ulid(), "lost", "dev:phone", &Default::default())
            .unwrap();
        let base = device.oplog_vv();
        let msg_id = ulid();
        device
            .append_user_message(&msg_id, "with picture", "dev:phone", &picture_opts(&other))
            .unwrap();
        let bytes = device.export_updates_since(&base);
        let room = arbos_core::wire::room_name(people::PEOPLE_PROJECT, &g);
        state
            .db
            .append_row(&room, "dev:phone", &ulid(), &bytes)
            .unwrap();
        let doc = crate::matrix::backfill::load_doc(&state, &g, None)
            .unwrap()
            .unwrap();
        assert!(
            doc.message(&msg_id).is_none(),
            "the doc cannot apply the row"
        );

        let me = Sender::user(&state, &other);
        on_announce_waiting(
            &state,
            &ChatRef::people(&g),
            &me,
            &msg_id,
            "with picture",
            None,
            &[],
            std::time::Duration::from_millis(400),
        );
        let events = events_eventually(&state, &pid).await;
        assert_eq!(events.len(), 1);
        let msg = &events[0].1["message"];
        assert_eq!(msg["text"], "with picture");
        assert!(msg.get("attachments").is_none(), "no empty list: {msg}");
        assert_eq!(msg["attachments_unknown"], true);

        // With no wait at all the answer is the same, delivered at once.
        let next = ulid();
        on_announce(&state, &ChatRef::people(&g), &me, &next, "x", None, &[]);
        let events = state.db.asp_events_after(&pid, 0, 10).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].1["message"]["attachments_unknown"], true);
        assert!(events[1].1["message"].get("attachments").is_none());
    }

    /// Arbonites, 2026-10-28 (01M627S7…, 01M627VG…): a box wrote an agent-to-agent hand-off row
    /// and a background job's host row into a group — no words, bare `sender: "agent"`. The
    /// outside agent's history and wake context leave them out; a stream it ends with no words
    /// is refused and its row removed, and a stream with words still posts.
    #[tokio::test]
    async fn blank_rows_stay_out_and_empty_streams_are_refused() {
        use crate::people::tests_support::{group, person, project_of};
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;
        use arbos_core::doc::UserMessageOpts;
        use axum::body::to_bytes;

        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 7401);
        let (other, _) = person(&state, 7402);
        let pid = project_of(&state, &owner, "Grok Bot");
        let key = mint_key();
        state
            .db
            .insert_outside_agent(&pid, &owner, "grok", &hash(&key))
            .unwrap();
        let g = group(&state, &[&owner, &other]);
        state
            .db
            .add_member(&g, &format!("agent:{pid}"), "agent", &owner, &json!({}))
            .unwrap();
        let (hi, ask) = (ulid(), ulid());
        let (h2, a2, from) = (hi.clone(), ask.clone(), other.clone());
        crate::notices::write_doc(&state, None, &g, move |doc| {
            let user = |id: &str, text: &str| {
                doc.append_user_message(
                    id,
                    text,
                    "dev:test",
                    &UserMessageOpts {
                        sender: Some(format!("user:{from}")),
                        ..Default::default()
                    },
                )
            };
            user(&h2, "morning all")?;
            doc.start_assistant_message("handoff", "", "box:bubble");
            doc.set_message_fields(
                "handoff",
                &[
                    ("turn", json!({ "kind": "spawn" })),
                    ("subagentName", json!("Bob")),
                ],
            );
            doc.add_hidden_part("handoff", "agent_message_sent", "Hey Bob", &json!({}));
            doc.finish_message("handoff", MessageStatus::Done);
            doc.start_assistant_message("job", "", "box:bubble");
            doc.set_message_fields(
                "job",
                &[
                    ("job", json!({ "id": "j1" })),
                    ("hidden", json!(true)),
                    ("worker", json!(true)),
                ],
            );
            doc.finish_message("job", MessageStatus::Done);
            user(&a2, "@grok-bot what were those two blank lines?")?;
            Ok(())
        })
        .await
        .unwrap();

        let agent = state
            .db
            .outside_agent_by_key_hash(&hash(&key))
            .unwrap()
            .unwrap();
        let project = state.db.project(&pid).unwrap().unwrap();
        let me = AgentPrincipal {
            agent,
            project: project.clone(),
        };
        let resp = history(
            State(state.clone()),
            me.clone(),
            Path(g.clone()),
            Query(HistoryQuery {
                before: None,
                limit: None,
            }),
        )
        .await;
        let body = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        let ids: Vec<&str> = v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [hi.as_str(), ask.as_str()]);
        let chat = chat_json_for(&state, &project, &g).unwrap();
        let ctx = crate::outside_sidecar::context_for(&state, &project, &chat, Some(&ask), 10);
        let recent = ctx["recent"].as_array().unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0]["text"], "morning all");

        // An empty stream: refused, and no row is left behind.
        let started = apply_event(&state, &me, &json!({ "type": "message.start", "chat": g }))
            .await
            .unwrap();
        let empty = started["message"].as_str().unwrap().to_owned();
        let r = apply_event(&state, &me, &json!({ "type": "message.end", "chat": g })).await;
        assert_eq!(r.unwrap_err().0, "bad_request");
        let doc = crate::matrix::backfill::load_doc(&state, &g, None)
            .unwrap()
            .unwrap();
        assert!(doc.message(&empty).is_none());
        // Ending another writer's blank row by its id leaves it in place and posts nothing.
        let r = apply_event(
            &state,
            &me,
            &json!({ "type": "message.end", "chat": g, "message": "handoff" }),
        )
        .await;
        assert!(r.is_err());
        let doc = crate::matrix::backfill::load_doc(&state, &g, None)
            .unwrap()
            .unwrap();
        assert!(doc.message("handoff").is_some());

        // A stream with words still posts.
        apply_event(&state, &me, &json!({ "type": "message.start", "chat": g }))
            .await
            .unwrap();
        apply_event(
            &state,
            &me,
            &json!({ "type": "message.end", "chat": g, "text": "Those were hand-off rows." }),
        )
        .await
        .unwrap();
        let doc = crate::matrix::backfill::load_doc(&state, &g, None)
            .unwrap()
            .unwrap();
        let last = doc.messages().pop().unwrap();
        assert_eq!(last.text(), "Those were hand-off rows.");
        assert_eq!(last.rest["sender"], format!("agent:{pid}"));
    }

    /// `message.delta` / `message.end` reach only a row the same agent started and has not
    /// finished, and a stream stops at [`MESSAGE_MAX`]; a refused event leaves the row as it was.
    #[tokio::test]
    async fn stream_events_touch_only_own_open_messages() {
        use crate::people::tests_support::{group, person, project_of};
        use crate::state::test_support::test_state;
        use crate::state::AuthMode;
        use arbos_core::doc::UserMessageOpts;

        let state = test_state(AuthMode::Github);
        let (owner, _) = person(&state, 7201);
        let (other, _) = person(&state, 7202);
        let pid = project_of(&state, &owner, "Bubble Rig");
        let key = mint_key();
        state
            .db
            .insert_outside_agent(&pid, &owner, "custom", &hash(&key))
            .unwrap();
        let g = group(&state, &[&owner, &other]);
        state
            .db
            .add_member(&g, &format!("agent:{pid}"), "agent", &owner, &json!({}))
            .unwrap();
        let me = AgentPrincipal {
            agent: state
                .db
                .outside_agent_by_key_hash(&hash(&key))
                .unwrap()
                .unwrap(),
            project: state.db.project(&pid).unwrap().unwrap(),
        };

        // A person's finished row and another agent's row that is still streaming.
        let (human, box_row) = (ulid(), ulid());
        let (h2, b2, o2) = (human.clone(), box_row.clone(), other.clone());
        crate::notices::write_doc(&state, None, &g, move |doc| {
            doc.append_user_message(
                &h2,
                "the numbers are 14",
                "dev:other",
                &UserMessageOpts {
                    sender: Some(format!("user:{o2}")),
                    ..Default::default()
                },
            )?;
            let part = doc.start_assistant_message(&b2, "", "box:someone-else");
            doc.append_text(&b2, &part, "thinking");
            Ok(())
        })
        .await
        .unwrap();
        let row = |id: &str| {
            crate::matrix::backfill::load_doc(&state, &g, None)
                .unwrap()
                .unwrap()
                .message(id)
                .unwrap()
        };
        let code = |r: EventResult| r.unwrap_err().0;

        // Another member's message: refused, untouched — with or without an open stream.
        for (id, before, status) in [
            (&human, "the numbers are 14", "done"),
            (&box_row, "thinking", "streaming"),
        ] {
            let ev = |ty: &str, text: &str| json!({ "type": ty, "chat": g, "message": id, "text": text });
            assert_eq!(
                code(apply_event(&state, &me, &ev("message.delta", " (edited)")).await),
                "forbidden"
            );
            assert_eq!(
                code(apply_event(&state, &me, &ev("message.end", "rewritten")).await),
                "forbidden"
            );
            let started = apply_event(&state, &me, &json!({ "type": "message.start", "chat": g }))
                .await
                .unwrap();
            assert_eq!(
                code(apply_event(&state, &me, &ev("message.end", "rewritten")).await),
                "forbidden"
            );
            let r = row(id);
            assert_eq!(r.text(), before);
            assert_eq!(r.status, status);
            apply_event(
                &state,
                &me,
                &json!({ "type": "message.end", "chat": g, "message": started["message"], "text": "ok" }),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            code(
                apply_event(
                    &state,
                    &me,
                    &json!({ "type": "message.delta", "chat": g, "message": "nope", "text": "x" }),
                )
                .await
            ),
            "not_found"
        );

        // Its own finished messages: posted, and streamed then ended.
        let posted = apply_event(
            &state,
            &me,
            &json!({ "type": "message.post", "chat": g, "text": "done here" }),
        )
        .await
        .unwrap()["message"]
            .as_str()
            .unwrap()
            .to_owned();
        let streamed = apply_event(
            &state,
            &me,
            &json!({ "type": "message.start", "chat": g, "text": "short" }),
        )
        .await
        .unwrap()["message"]
            .as_str()
            .unwrap()
            .to_owned();
        apply_event(&state, &me, &json!({ "type": "message.end", "chat": g }))
            .await
            .unwrap();
        for (id, before) in [(&posted, "done here"), (&streamed, "short")] {
            for ty in ["message.delta", "message.end"] {
                let ev = json!({ "type": ty, "chat": g, "message": id, "text": "late" });
                assert_eq!(code(apply_event(&state, &me, &ev).await), "closed");
            }
            assert_eq!(row(id).text(), before);
            assert_eq!(row(id).status, "done");
        }
        // No open stream is left behind for an implicit delta to land in.
        assert_eq!(
            code(
                apply_event(
                    &state,
                    &me,
                    &json!({ "type": "message.delta", "chat": g, "text": "x" }),
                )
                .await
            ),
            "bad_request"
        );

        // The total cap: deltas up to MESSAGE_MAX land, one byte more is refused, end still works.
        let open = apply_event(&state, &me, &json!({ "type": "message.start", "chat": g }))
            .await
            .unwrap()["message"]
            .as_str()
            .unwrap()
            .to_owned();
        let chunk = "a".repeat(TEXT_MAX);
        for _ in 0..MESSAGE_MAX / TEXT_MAX {
            apply_event(
                &state,
                &me,
                &json!({ "type": "message.delta", "chat": g, "text": chunk }),
            )
            .await
            .unwrap();
        }
        let over = json!({ "type": "message.delta", "chat": g, "message": open, "text": "b" });
        assert_eq!(code(apply_event(&state, &me, &over).await), "too_large");
        let r = row(&open);
        assert_eq!(r.text().len(), MESSAGE_MAX);
        assert_eq!(r.status, "streaming");
        apply_event(&state, &me, &json!({ "type": "message.end", "chat": g }))
            .await
            .unwrap();
        let r = row(&open);
        assert_eq!(r.text().len(), MESSAGE_MAX);
        assert_eq!(r.status, "done");
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
            scopes: json!({}),
            wake_url: None,
            wake_auth_set: false,
            wake_last: None,
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
